use super::*;

fn worst_case() -> Board {
    let name = |i: usize| format!("agent-name-{i:02}-{}", "x".repeat(20));
    Board {
        version: VERSION,
        at: 1_700_000_000,
        waiting: (0..2)
            .map(|i| Waiting {
                name: name(i),
                space: Some(format!("space-{}", "y".repeat(34))),
                since: 1_700_000_000 - 600,
                error: i == 1,
            })
            .collect(),
        more: 7,
        running: (0..8).map(|i| name(10 + i)).collect(),
        running_more: 20,
        idle: 40,
        finished: Some(name(30)),
        finished_at: Some(1_700_000_000),
    }
}

#[test]
fn a_board_opens_only_with_its_devices_key() {
    let device = random_secret();
    let plain = serde_json::to_vec(&worst_case()).unwrap();
    let blob = seal(&device.public_key().to_sec1_bytes(), &plain).unwrap();
    assert_eq!(open(&device, &blob).unwrap(), plain);
    assert_eq!(open(&random_secret(), &blob), Err(LiveError::Open));
    let mut raw = B64.decode(&blob).unwrap();
    *raw.last_mut().unwrap() ^= 1;
    assert_eq!(open(&device, &B64.encode(raw)), Err(LiveError::Open));
}

#[test]
fn a_worst_case_board_fits_one_push() {
    let device = random_secret();
    let plain = serde_json::to_vec(&worst_case()).unwrap();
    let blob = seal(&device.public_key().to_sec1_bytes(), &plain).unwrap();
    assert!(blob.len() < MAX_BLOB_CHARS / 2, "{} chars", blob.len());
    assert_eq!(
        seal(&device.public_key().to_sec1_bytes(), &[b'x'; 4000]),
        Err(LiveError::TooBig)
    );
    assert_eq!(seal(b"nope", b"{}"), Err(LiveError::BadKey));
}

/// Fixed keys and nonce pin the format the phones open; `SWITCHBOARD_WRITE_LIVE_VECTOR` rewrites it.
#[test]
fn the_phones_open_the_shared_vector() {
    let path = "tests/fixtures/app-v1/live-vector.json";
    let device = SecretKey::from_slice(&[0x55; 32]).unwrap();
    let eph = SecretKey::from_slice(&[0x66; 32]).unwrap();
    let plain = serde_json::to_string(&worst_case()).unwrap();
    let blob = seal_with(
        &eph,
        [0x77; 12],
        &device.public_key().to_sec1_bytes(),
        plain.as_bytes(),
    )
    .unwrap();
    let v = serde_json::json!({
        "device_secret_hex": hex::encode([0x55; 32]),
        "device_public": B64.encode(device.public_key().to_sec1_bytes()),
        "plain": plain,
        "blob": blob,
    });
    if std::env::var("SWITCHBOARD_WRITE_LIVE_VECTOR").is_ok() {
        std::fs::write(path, serde_json::to_string_pretty(&v).unwrap() + "\n").unwrap();
        return;
    }
    let want: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(v, want);
}

fn seen(name: &str, status: &str) -> Seen {
    Seen {
        machine: "mac".into(),
        name: name.into(),
        label: name.into(),
        space: Some("atlas".into()),
        status: status.into(),
        error: false,
        needs_you_since: None,
    }
}

#[test]
fn the_board_sorts_agents_by_what_they_need() {
    let mut t = Tracker::default();
    let mut err = seen("migrate", "done");
    err.error = true;
    let mut asked = seen("billing", "working");
    asked.needs_you_since = Some(900);
    let b = t.observe(
        1000,
        &[
            seen("checkout", "blocked"),
            seen("api", "working"),
            seen("docs", "idle"),
            err,
            asked,
        ],
    );
    assert_eq!(
        b.waiting
            .iter()
            .map(|w| (w.name.as_str(), w.since, w.error))
            .collect::<Vec<_>>(),
        [("billing", 900, false), ("checkout", 1000, false)]
    );
    assert_eq!(b.more, 1, "the error is counted beyond the two named");
    assert_eq!(b.running, ["api"]);
    assert_eq!(b.idle, 1);
    assert_eq!(
        Signal::between(None, &b),
        Signal {
            alert: true,
            urgent: true
        }
    );

    // Still waiting a minute later: the same since, and no new alert.
    let b2 = t.observe(1060, &[seen("checkout", "blocked"), seen("api", "working")]);
    assert_eq!(b2.waiting[0].since, 1000);
    let b3 = t.observe(1070, &[seen("checkout", "blocked"), seen("api", "working")]);
    assert_eq!(Signal::between(Some(&b2), &b3), Signal::default());
    // One more waiting, even beyond the named two, alerts again.
    let b4 = Board {
        more: b3.more + 1,
        ..b3.clone()
    };
    assert!(Signal::between(Some(&b3), &b4).alert);
}

#[test]
fn a_finished_agent_shows_for_a_minute_and_all_done_is_urgent() {
    let mut t = Tracker::default();
    let busy = t.observe(0, &[seen("api", "working"), seen("docs", "idle")]);
    let b = t.observe(10, &[seen("api", "idle"), seen("docs", "idle")]);
    assert_eq!(
        (b.finished.as_deref(), b.finished_at),
        (Some("api"), Some(10))
    );
    let s = Signal::between(Some(&busy), &b);
    assert!(s.urgent && !s.alert, "all done is urgent but silent");
    let b = t.observe(69, &[seen("api", "idle"), seen("docs", "idle")]);
    assert_eq!(b.finished.as_deref(), Some("api"));
    let calm = t.observe(70, &[seen("api", "idle"), seen("docs", "idle")]);
    assert_eq!(calm.finished, None);
    assert_eq!(Signal::between(Some(&b), &calm), Signal::default());
}

#[test]
fn many_running_agents_are_counted_beyond_the_named_ones() {
    let mut t = Tracker::default();
    let agents: Vec<_> = (0..12)
        .map(|i| seen(&format!("a{i:02}"), "working"))
        .collect();
    let b = t.observe(0, &agents);
    assert_eq!(b.running.len(), MAX_RUNNING_NAMES);
    assert_eq!(b.running_more, 4);
    let b2 = t.observe(5, &agents);
    assert!(b.same(&b2) && b.at != b2.at);
    assert!(!b.is_empty() && Board::default().is_empty());
}

#[test]
fn the_card_names_agents_as_the_app_does() {
    assert_eq!(
        agent_label("hc/phone-features", Some("whatever"), Some("claude")),
        "phone-features"
    );
    assert_eq!(
        agent_label("crm/w1:p2", Some("  Fix the import "), Some("codex")),
        "Fix the import"
    );
    assert_eq!(agent_label("crm/w1:p2", Some(" "), Some("codex")), "Codex");
    assert_eq!(agent_label("w1:p3", None, None), "w1:p3");
    assert_eq!(agent_label("plain", None, None), "plain");
    // The board shows labels; the name only identifies.
    let mut t = Tracker::default();
    let mut a = seen("crm/w1:p2", "working");
    a.label = "Fix the import".into();
    assert_eq!(t.observe(0, &[a]).running, ["Fix the import"]);
}
