use std::sync::atomic::{AtomicBool, Ordering};

use super::*;

fn fixture(dir: &str, name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/dialogs/{dir}/{name}.txt",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// Moves `marker` to row `n` of a captured screen, as the CLI redraws it after an arrow.
fn select(text: &str, marker: char, n: u32) -> String {
    let mark = format!("{marker} ");
    text.lines()
        .map(|l| {
            let t = l.trim_start();
            let indent = &l[..l.len() - t.len()];
            let (rest, marked) = match t.strip_prefix(&mark) {
                Some(r) => (r, true),
                None => (t, false),
            };
            let number = rest
                .split_once(". ")
                .and_then(|(d, _)| d.parse::<u32>().ok());
            match number {
                Some(k) if k == n && !marked => {
                    format!("{}{mark}{rest}", &indent[..indent.len() - 2])
                }
                Some(k) if k != n && marked => format!("{indent}  {rest}"),
                _ => l.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

type OnArrow = Box<dyn FnMut(&mut State) + Send>;

struct State {
    process: Process,
    /// The dialog's screen with row `selected` marked, until Enter.
    dialog: Option<String>,
    marker: char,
    selected: u32,
    rows: u32,
    after: String,
    /// Arrows the CLI ignores, so the selection never moves.
    stuck: bool,
    sent: Vec<Vec<&'static str>>,
    /// Applied when Enter arrives, before it lands: a dialog replaced in between.
    replace_on_enter: Option<String>,
    /// Which screen Enter landed on.
    entered: Vec<String>,
    /// Changes on the first arrow.
    on_arrow: Option<OnArrow>,
    call: Option<String>,
    /// A group's later screens with their row counts: Enter shows the next, still blocked.
    next: std::collections::VecDeque<(String, u32)>,
    /// The screen a left arrow shows, with its row count.
    prev: Option<(String, u32)>,
}

struct Fake(Mutex<State>);

impl Fake {
    fn new(kind: &str, dialog: String, marker: char, rows: u32, after: String) -> Arc<Self> {
        Arc::new(Self(Mutex::new(State {
            process: Process {
                kind: Some(kind.into()),
                terminal_id: Some("term_1".into()),
                agent_session: None,
                state_change_seq: 7,
                blocked: true,
            },
            dialog: Some(dialog),
            marker,
            selected: 1,
            rows,
            after,
            stuck: false,
            sent: Vec::new(),
            replace_on_enter: None,
            entered: Vec::new(),
            on_arrow: None,
            call: None,
            next: Default::default(),
            prev: None,
        })))
    }

    fn codex() -> Arc<Self> {
        Self::new(
            "codex",
            fixture("codex-0.165.0", "permission-command"),
            '›',
            3,
            fixture("codex-0.165.0", "after-deny-command"),
        )
    }

    fn sent(&self) -> Vec<Vec<&'static str>> {
        self.0.lock().unwrap().sent.clone()
    }

    fn enters(&self) -> usize {
        self.sent().iter().filter(|k| k.contains(&"enter")).count()
    }
}

impl DialogIo for Arc<Fake> {
    async fn process(&self) -> Result<Process, String> {
        Ok(self.0.lock().unwrap().process.clone())
    }

    async fn screen(&self) -> Result<String, String> {
        let s = self.0.lock().unwrap();
        Ok(match &s.dialog {
            Some(d) => select(d, s.marker, s.selected),
            None => s.after.clone(),
        })
    }

    async fn pending_call(&self) -> Option<String> {
        self.0.lock().unwrap().call.clone()
    }

    async fn send(&self, keys: &[&'static str]) -> Result<(), String> {
        let mut s = self.0.lock().unwrap();
        s.sent.push(keys.to_vec());
        for k in keys {
            match *k {
                "down" | "up" => {
                    if let Some(mut f) = s.on_arrow.take() {
                        f(&mut s);
                    }
                    if !s.stuck {
                        s.selected = match *k {
                            "down" => (s.selected + 1).min(s.rows),
                            _ => (s.selected - 1).max(1),
                        };
                    }
                }
                "enter" => {
                    if let Some(next) = s.replace_on_enter.take() {
                        s.dialog = Some(next);
                        s.selected = 1;
                    }
                    let landed = s
                        .dialog
                        .as_deref()
                        .map(|d| select(d, s.marker, s.selected))
                        .unwrap_or_default();
                    s.entered.push(landed);
                    if let Some((screen, rows)) = s.next.pop_front() {
                        (s.dialog, s.rows, s.selected) = (Some(screen), rows, 1);
                        continue;
                    }
                    s.dialog = None;
                    s.process.state_change_seq += 1;
                }
                "left" => {
                    if let Some((screen, rows)) = s.prev.take() {
                        (s.dialog, s.rows, s.selected) = (Some(screen), rows, 1);
                    }
                }
                other => panic!("unexpected key {other}"),
            }
        }
        Ok(())
    }
}

fn never() -> Option<&'static str> {
    None
}

async fn seen(io: &Arc<Fake>) -> (Seen, DialogView) {
    let s = look(io).await.unwrap();
    let v = view(&s, &never, &Answered::default()).unwrap();
    (s, v)
}

fn request(v: &DialogView, index: usize) -> AnswerRequest {
    let o = &v.options[index];
    AnswerRequest {
        occurrence: v.occurrence.clone(),
        option: o.id.clone(),
        label_sha256: o.label_sha256.clone(),
        deny_only: false,
    }
}

#[tokio::test(start_paused = true)]
async fn arrows_then_one_enter_after_the_selection_is_verified() {
    let io = Fake::codex();
    let (_, v) = seen(&io).await;
    assert!(v.answerable, "{v:?}");
    assert_eq!(
        v.options.iter().map(|o| o.offered).collect::<Vec<_>>(),
        [true, true, true]
    );
    let answered = Answered::default();
    answer(&io, &never, &answered, &request(&v, 2))
        .await
        .unwrap();
    assert_eq!(io.sent(), vec![vec!["down", "down"], vec!["enter"]]);
    assert!(io.0.lock().unwrap().entered[0].contains("› 3. No, and tell Codex"));
    // A replay of the same answer sends nothing.
    let again = answer(&io, &never, &answered, &request(&v, 2))
        .await
        .unwrap_err();
    assert_eq!(again.code, ErrorCode::DialogChanged);
    assert_eq!(io.enters(), 1);
}

#[tokio::test(start_paused = true)]
async fn the_selected_option_needs_no_arrows() {
    let io = Fake::codex();
    let (_, v) = seen(&io).await;
    answer(&io, &never, &Answered::default(), &request(&v, 0))
        .await
        .unwrap();
    assert_eq!(io.sent(), vec![vec!["enter"]]);
}

#[tokio::test(start_paused = true)]
async fn a_changed_screen_gets_no_keys() {
    let io = Fake::codex();
    let (_, v) = seen(&io).await;
    io.0.lock().unwrap().dialog = Some(fixture("codex-0.165.0", "permission-edit"));
    let e = answer(&io, &never, &Answered::default(), &request(&v, 2))
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::DialogChanged);
    assert!(io.sent().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_selection_that_never_moves_gets_no_enter_and_may_be_retried() {
    let io = Fake::codex();
    io.0.lock().unwrap().stuck = true;
    let (_, v) = seen(&io).await;
    let answered = Answered::default();
    let e = answer(&io, &never, &answered, &request(&v, 2))
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::DialogChanged);
    assert_eq!(io.sent(), vec![vec!["down", "down"]]);
    assert!(
        !answered.blocks(&v.occurrence),
        "no Enter, so it may be tried again"
    );
}

#[tokio::test(start_paused = true)]
async fn a_changed_process_between_the_arrows_and_enter_stops_everything() {
    for change in [
        |s: &mut State| s.process.state_change_seq += 1,
        |s: &mut State| s.process.terminal_id = Some("term_2".into()),
        |s: &mut State| s.process.agent_session = Some("other".into()),
    ] {
        let io = Fake::codex();
        let (_, v) = seen(&io).await;
        io.0.lock().unwrap().on_arrow = Some(Box::new(change));
        let e = answer(&io, &never, &Answered::default(), &request(&v, 2))
            .await
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::DialogChanged);
        assert_eq!(io.enters(), 0);
        assert!(!io.sent().iter().flatten().any(|k| *k == "esc"));
    }
}

#[tokio::test(start_paused = true)]
async fn two_answers_at_once_send_one_enter() {
    let io = Fake::codex();
    let (_, v) = seen(&io).await;
    let answered = Arc::new(Answered::default());
    let locks = Arc::new(PaneLocks::default());
    let run = |index: usize| {
        let (io, answered, locks, req) = (
            io.clone(),
            answered.clone(),
            locks.clone(),
            request(&v, index),
        );
        tokio::spawn(async move {
            let _g = locks.lock(None, "w1:p1").await;
            answer(&io, &never, &answered, &req).await
        })
    };
    let (a, b) = (run(0), run(2));
    let results = [a.await.unwrap(), b.await.unwrap()];
    assert_eq!(
        results.iter().filter(|r| r.is_ok()).count(),
        1,
        "{results:?}"
    );
    assert_eq!(io.enters(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_hook_call_that_arrives_during_the_arrows_stops_the_enter() {
    let io = Fake::codex();
    let (_, v) = seen(&io).await;
    let pending = Arc::new(AtomicBool::new(false));
    let flag = pending.clone();
    io.0.lock().unwrap().on_arrow = Some(Box::new(move |_| flag.store(true, Ordering::SeqCst)));
    let refuse = move || pending.load(Ordering::SeqCst).then_some("hook_pending");
    let e = answer(&io, &refuse, &Answered::default(), &request(&v, 2))
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::DialogChanged);
    assert_eq!(io.enters(), 0);
}

#[tokio::test(start_paused = true)]
async fn refused_panes_and_options_get_no_keys() {
    let io = Fake::codex();
    let (s, v) = seen(&io).await;
    let hooked = || Some("hooked");
    let hv = view(&s, &hooked, &Answered::default()).unwrap();
    assert_eq!(
        (hv.answerable, hv.reason.as_deref()),
        (false, Some("hooked"))
    );
    let answered = Answered::default();
    let cases = [
        (
            answer(&io, &hooked, &answered, &request(&v, 2)).await,
            ErrorCode::DialogChanged,
        ),
        (
            answer(
                &io,
                &never,
                &answered,
                &AnswerRequest {
                    deny_only: true,
                    ..request(&v, 0)
                },
            )
            .await,
            ErrorCode::ApproveNotAllowed,
        ),
        (
            answer(
                &io,
                &never,
                &answered,
                &AnswerRequest {
                    label_sha256: "0".repeat(64),
                    ..request(&v, 0)
                },
            )
            .await,
            ErrorCode::DialogChanged,
        ),
    ];
    for (i, (r, code)) in cases.into_iter().enumerate() {
        assert_eq!(r.unwrap_err().code, code, "case {i}");
    }
    assert!(io.sent().is_empty());
    // The deny needs no approve right.
    let req = AnswerRequest {
        deny_only: true,
        ..request(&v, 2)
    };
    answer(&io, &never, &answered, &req).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_free_text_row_is_shown_but_never_chosen() {
    let io = Fake::new(
        "claude",
        fixture("claude-2.1.312", "question-single"),
        '❯',
        5,
        String::new(),
    );
    let (_, v) = seen(&io).await;
    assert_eq!(
        v.options.iter().map(|o| o.offered).collect::<Vec<_>>(),
        [true, true, true, false, false]
    );
    let e = answer(&io, &never, &Answered::default(), &request(&v, 3))
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::BadRequest);
    assert!(io.sent().is_empty());
}

#[tokio::test(start_paused = true)]
async fn read_only_dialogs_are_shown_but_not_answerable() {
    let multi = Fake::new(
        "claude",
        fixture("claude-2.1.312", "question-multiselect"),
        '❯',
        5,
        String::new(),
    );
    let (_, v) = seen(&multi).await;
    assert_eq!(
        (v.answerable, v.reason.as_deref()),
        (false, Some("read_only"))
    );
    assert!(v.options.iter().all(|o| !o.offered));
}

#[tokio::test(start_paused = true)]
async fn a_claude_question_is_answered_across_its_description_lines() {
    let io = Fake::new(
        "claude",
        fixture("claude-2.1.312", "question-single"),
        '❯',
        5,
        String::new(),
    );
    let (_, v) = seen(&io).await;
    assert_eq!(v.kind, crate::agents::dialog::DialogKind::Question);
    assert_eq!(v.options[2].label, "Blue");
    answer(&io, &never, &Answered::default(), &request(&v, 2))
        .await
        .unwrap();
    assert_eq!(io.sent(), vec![vec!["down", "down"], vec!["enter"]]);
    assert!(io.0.lock().unwrap().entered[0].contains("❯ 3. Blue"));
}

#[tokio::test(start_paused = true)]
async fn identical_dialogs_differ_by_their_call_ids() {
    let io = Fake::codex();
    io.0.lock().unwrap().call = Some("call_a".into());
    let a = look(&io).await.unwrap().occurrence;
    io.0.lock().unwrap().call = Some("call_b".into());
    let b = look(&io).await.unwrap().occurrence;
    assert_ne!(a, b);
    // The marker and footer follow the selection; the occurrence doesn't.
    io.0.lock().unwrap().selected = 3;
    assert_eq!(look(&io).await.unwrap().occurrence, b);
}

/// Documents the risk the owner accepted: a dialog replaced after the last read and before
/// the Enter lands gets that Enter. Herdr can't make the read and the key one step.
#[tokio::test(start_paused = true)]
async fn a_dialog_replaced_between_the_read_and_enter_gets_the_enter() {
    let io = Fake::codex();
    let (_, v) = seen(&io).await;
    let next = fixture("codex-0.165.0", "permission-edit");
    io.0.lock().unwrap().replace_on_enter = Some(next);
    let r = answer(&io, &never, &Answered::default(), &request(&v, 0)).await;
    assert!(r.is_ok());
    let entered = io.0.lock().unwrap().entered.clone();
    assert_eq!(entered.len(), 1);
    assert!(entered[0].contains("Would you like to make the following edits?"));
}

#[test]
fn attempts_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("answers.json");
    let a = Answered::load(path.clone());
    a.begin("o1").unwrap();
    a.begin("o2").unwrap();
    a.settle("o2", Attempt::Aborted);
    let b = Answered::load(path);
    assert!(b.blocks("o1"), "a started attempt is never retried");
    assert!(!b.blocks("o2"));
    assert!(b.begin("o1").is_err());
}

#[test]
fn whatsapp_never_reaches_the_key_path() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/whatsapp");
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        for needle in [
            concat!("agent_send", "_keys"),
            concat!("Op::", "Answer"),
            concat!("Answer", "Request"),
            concat!("runner::", "dialogs"),
        ] {
            assert!(!text.contains(needle), "{} uses {needle}", path.display());
        }
    }
}

/// Answers what the screen shows now by its `index`th option, returning what the phone saw.
async fn pick(io: &Arc<Fake>, answered: &Answered, index: usize) -> DialogView {
    let s = look(io).await.unwrap();
    let v = view(&s, &never, answered).unwrap();
    assert!(v.answerable, "{v:?}");
    answer(io, &never, answered, &request(&v, index))
        .await
        .unwrap();
    v
}

#[tokio::test(start_paused = true)]
async fn a_claude_group_is_answered_one_question_at_a_time_then_submitted() {
    let dir = "core/claude-2.1.325";
    let io = Fake::new(
        "claude",
        fixture(dir, "question-multi-1"),
        '❯',
        4,
        fixture(dir, "question-multi-after"),
    );
    io.0.lock().unwrap().next = [
        (fixture(dir, "question-multi-2"), 4),
        (fixture(dir, "question-multi-3"), 5),
        (fixture(dir, "question-multi-review"), 2),
    ]
    .into();
    let answered = Answered::default();
    let size = pick(&io, &answered, 1).await;
    let drink = pick(&io, &answered, 0).await;
    let colour = pick(&io, &answered, 2).await;
    let review = pick(&io, &answered, 0).await;
    assert_eq!(
        io.sent(),
        vec![
            vec!["down"],
            vec!["enter"],
            vec!["enter"],
            vec!["down", "down"],
            vec!["enter"],
            vec!["enter"],
        ]
    );
    let entered = io.0.lock().unwrap().entered.clone();
    for (screen, row) in
        entered
            .iter()
            .zip(["❯ 2. Large", "❯ 1. Tea", "❯ 3. Blue", "❯ 1. Submit answers"])
    {
        assert!(screen.contains(row), "{row}");
    }
    // Each step is its own occurrence, so an answer for one never lands on another.
    let seen = [&size, &drink, &colour, &review].map(|v| v.occurrence.clone());
    assert_eq!(
        seen.iter().collect::<std::collections::HashSet<_>>().len(),
        4
    );
    assert!(review.options[1].is_deny && !review.options[0].is_deny);
    assert!(look(&io).await.unwrap().dialog.is_none());
}

#[tokio::test(start_paused = true)]
async fn an_answer_for_one_question_of_a_group_never_lands_on_the_next() {
    let dir = "core/claude-2.1.325";
    let io = Fake::new(
        "claude",
        fixture(dir, "question-multi-1"),
        '❯',
        4,
        String::new(),
    );
    let (_, first) = seen(&io).await;
    // Answered at the Mac meanwhile: the second question is up.
    io.0.lock().unwrap().dialog = Some(fixture(dir, "question-multi-2"));
    let e = answer(&io, &never, &Answered::default(), &request(&first, 1))
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::DialogChanged);
    assert!(io.sent().is_empty());
}

#[tokio::test(start_paused = true)]
async fn cancelling_a_claude_review_needs_no_approve_right() {
    let dir = "core/claude-2.1.325";
    let review = || {
        Fake::new(
            "claude",
            fixture(dir, "question-multi-review"),
            '❯',
            2,
            fixture(dir, "question-multi-after"),
        )
    };
    let io = review();
    let (_, v) = seen(&io).await;
    let deny_only = |index| AnswerRequest {
        deny_only: true,
        ..request(&v, index)
    };
    let e = answer(&io, &never, &Answered::default(), &deny_only(0))
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::ApproveNotAllowed);
    assert!(io.sent().is_empty());
    answer(&io, &never, &Answered::default(), &deny_only(1))
        .await
        .unwrap();
    assert_eq!(io.sent(), vec![vec!["down"], vec!["enter"]]);
}

#[tokio::test(start_paused = true)]
async fn a_codex_group_is_answered_one_question_at_a_time() {
    let dir = "core/codex-0.169.0";
    let io = Fake::new(
        "codex",
        fixture(dir, "question-multi-1"),
        '›',
        3,
        fixture(dir, "question-multi-after"),
    );
    io.0.lock().unwrap().next = [
        (fixture(dir, "question-multi-2"), 3),
        (fixture(dir, "question-multi-3"), 4),
    ]
    .into();
    let answered = Answered::default();
    let first = pick(&io, &answered, 1).await;
    assert_eq!(
        first.options.iter().map(|o| o.offered).collect::<Vec<_>>(),
        [true, true, false]
    );
    pick(&io, &answered, 0).await;
    pick(&io, &answered, 2).await;
    assert_eq!(
        io.sent(),
        vec![
            vec!["down"],
            vec!["enter"],
            vec!["enter"],
            vec!["down", "down"],
            vec!["enter"],
        ]
    );
    let entered = io.0.lock().unwrap().entered.clone();
    for (screen, row) in entered.iter().zip(["› 2. Large", "› 1. Tea", "› 3. Blue"]) {
        assert!(screen.contains(row), "{row}");
    }
    // The notes row is shown but never chosen.
    let io = Fake::new(
        "codex",
        fixture(dir, "question-single"),
        '›',
        3,
        fixture(dir, "question-single-after"),
    );
    let (_, v) = seen(&io).await;
    let e = answer(&io, &never, &Answered::default(), &request(&v, 2))
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::BadRequest);
    assert!(io.sent().is_empty());
}

#[tokio::test(start_paused = true)]
async fn only_the_choice_that_submits_a_group_needs_the_approve_right() {
    let unsigned = |v: &DialogView, index| AnswerRequest {
        deny_only: true,
        ..request(v, index)
    };
    let (claude, codex) = ("core/claude-2.1.325", "core/codex-0.169.0");
    // A question of a group, then the screen whose Enter submits, with their row counts.
    let cases = [
        (
            "claude",
            claude,
            '❯',
            ("question-multi-2", 4),
            ("question-multi-review", 2),
        ),
        (
            "codex",
            codex,
            '›',
            ("question-multi-2", 3),
            ("question-multi-3", 4),
        ),
        (
            "codex",
            codex,
            '›',
            ("question-multi-1", 3),
            ("question-single", 3),
        ),
    ];
    for (kind, dir, marker, (question, rows), (submits, submit_rows)) in cases {
        let io = Fake::new(kind, fixture(dir, question), marker, rows, String::new());
        io.0.lock().unwrap().next = [(fixture(dir, submits), submit_rows)].into();
        let (_, v) = seen(&io).await;
        assert!(v.options[1].provisional, "{kind} {question}");
        answer(&io, &never, &Answered::default(), &unsigned(&v, 1))
            .await
            .unwrap();
        assert_eq!(io.enters(), 1, "{kind} {question}");
        // The screen that submits them takes no unsigned choice but a deny.
        let (_, v) = seen(&io).await;
        assert!(v.options.iter().all(|o| !o.provisional), "{kind} {submits}");
        let e = answer(&io, &never, &Answered::default(), &unsigned(&v, 0))
            .await
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::ApproveNotAllowed, "{kind} {submits}");
        assert_eq!(io.enters(), 1, "{kind} {submits}");
    }
}

fn open_tab(v: &DialogView) -> Option<&str> {
    v.steps.iter().find(|s| s.current).map(|s| s.label.as_str())
}

#[tokio::test(start_paused = true)]
async fn one_left_arrow_shows_a_groups_previous_question() {
    let dir = "core/claude-2.1.325";
    let io = Fake::new(
        "claude",
        fixture(dir, "question-multi-review"),
        '❯',
        2,
        String::new(),
    );
    io.0.lock().unwrap().prev = Some((fixture(dir, "question-multi-3-revisit"), 5));
    let answered = Answered::default();
    let s = look(&io).await.unwrap();
    let review = view(&s, &never, &answered).unwrap();
    assert!(review.back);
    assert_eq!(open_tab(&review), Some("Submit"));
    back(&io, &never, &answered, &review.occurrence)
        .await
        .unwrap();
    assert_eq!(io.sent(), vec![vec!["left"]]);
    // Every tab is answered now, so the screen alone doesn't say which is open; the move does.
    let s = look(&io).await.unwrap();
    let colour = view(&s, &never, &answered).unwrap();
    assert_eq!(open_tab(&colour), Some("Colour"));
    assert!(colour.back && colour.answerable);
    assert!(
        view(&s, &never, &Answered::default())
            .unwrap()
            .steps
            .iter()
            .all(|s| !s.current)
    );
    // A tap repeated for the review now behind it moves nothing.
    let e = back(&io, &never, &answered, &review.occurrence)
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::DialogChanged);
    assert_eq!(io.sent().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn no_left_arrow_without_an_earlier_question_or_from_a_free_text_row() {
    let claude = "core/claude-2.1.325";
    // (agent, folder, screen, marker, rows, the row selected)
    let cases = [
        ("claude", claude, "question-multi-1", '❯', 4, 1),
        ("claude", claude, "question-single-long", '❯', 5, 1),
        ("claude", claude, "question-multi-2", '❯', 4, 3),
        ("claude", claude, "question-mixed-multiselect", '❯', 5, 1),
        ("codex", "core/codex-0.169.0", "question-multi-1", '›', 3, 1),
        ("codex", "core/codex-0.169.0", "question-single", '›', 3, 1),
        ("codex", "codex-0.165.0", "permission-command", '›', 3, 1),
    ];
    for (kind, dir, screen, marker, rows, selected) in cases {
        let io = Fake::new(kind, fixture(dir, screen), marker, rows, String::new());
        io.0.lock().unwrap().selected = selected;
        let s = look(&io).await.unwrap();
        let v = view(&s, &never, &Answered::default()).unwrap();
        assert!(!v.back, "{screen}");
        let e = back(&io, &never, &Answered::default(), &v.occurrence)
            .await
            .unwrap_err();
        assert_ne!(e.code, ErrorCode::OutcomeUnknown, "{screen}");
        assert!(io.sent().is_empty(), "{screen}");
    }
    // A hooked pane gets no key either.
    let io = Fake::new(
        "claude",
        fixture(claude, "question-multi-2"),
        '❯',
        4,
        String::new(),
    );
    let (_, v) = seen(&io).await;
    assert!(v.back);
    let hooked = || Some("hooked");
    let e = back(&io, &hooked, &Answered::default(), &v.occurrence)
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::DialogChanged);
    assert!(io.sent().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_question_gone_back_to_may_be_answered_again() {
    let dir = "core/codex-0.169.0";
    let io = Fake::new(
        "codex",
        fixture(dir, "question-multi-2"),
        '›',
        3,
        String::new(),
    );
    io.0.lock().unwrap().next = [(fixture(dir, "question-multi-3"), 4)].into();
    let answered = Answered::default();
    let second = pick(&io, &answered, 1).await;
    let s = look(&io).await.unwrap();
    let third = view(&s, &never, &answered).unwrap();
    assert_eq!(open_tab(&third), Some("3"));
    // The very same screen comes back after a left arrow.
    io.0.lock().unwrap().prev = Some((fixture(dir, "question-multi-2"), 3));
    back(&io, &never, &answered, &third.occurrence)
        .await
        .unwrap();
    let s = look(&io).await.unwrap();
    let again = view(&s, &never, &answered).unwrap();
    assert_eq!(again.occurrence, second.occurrence);
    assert!(again.answerable, "{again:?}");
    io.0.lock().unwrap().next = [(fixture(dir, "question-multi-3"), 4)].into();
    pick(&io, &answered, 0).await;
    assert_eq!(io.enters(), 2);
}

#[tokio::test(start_paused = true)]
async fn moving_the_selection_on_an_answered_codex_question_keeps_its_occurrence() {
    let dir = "core/codex-0.169.0";
    let at = |screen: String| Fake::new("codex", screen, '›', 3, String::new());
    // Codex counts the question as unanswered again once the selection moves off its answer.
    let moved = fixture(dir, "question-multi-2-revisit-down");
    assert!(moved.contains("Question 2/3 (2 unanswered)"));
    let (before, _) = seen(&at(moved.replace("(2 unanswered)", "(1 unanswered)"))).await;
    let (after, _) = seen(&at(moved)).await;
    assert_eq!(before.occurrence, after.occurrence);
    let (other, _) = seen(&at(fixture(dir, "question-multi-1"))).await;
    assert_ne!(before.occurrence, other.occurrence);
}

#[tokio::test(start_paused = true)]
async fn an_answer_and_a_step_back_return_what_the_screen_then_shows() {
    let dir = "core/claude-2.1.325";
    let io = Fake::new(
        "claude",
        fixture(dir, "question-multi-1"),
        '❯',
        4,
        String::new(),
    );
    io.0.lock().unwrap().next = [(fixture(dir, "question-multi-2"), 4)].into();
    let answered = Answered::default();
    let (_, first) = seen(&io).await;
    let after = answer(&io, &never, &answered, &request(&first, 1))
        .await
        .unwrap();
    let next = view(&after, &never, &answered).unwrap();
    assert_eq!(next.prompt, "Pick a drink");
    assert_eq!(open_tab(&next), Some("Drink"));
    assert!(next.answerable && next.back);
    io.0.lock().unwrap().prev = Some((fixture(dir, "question-multi-1"), 4));
    let before = back(&io, &never, &answered, &next.occurrence)
        .await
        .unwrap();
    assert_eq!(
        view(&before, &never, &answered).unwrap().prompt,
        "Pick a size"
    );
    // The last answer leaves no dialog to return.
    let last = Fake::codex();
    let (_, v) = seen(&last).await;
    let after = answer(&last, &never, &Answered::default(), &request(&v, 0))
        .await
        .unwrap();
    assert!(view(&after, &never, &Answered::default()).is_none());
}
