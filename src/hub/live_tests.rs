use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use p256::SecretKey;

use super::*;
use crate::live::{Waiting, open};
use crate::store::LiveRegistration;

fn key() -> SecretKey {
    SecretKey::from_slice(&[0x42; 32]).unwrap()
}

fn iphone(reg: LiveRegistration) -> LiveTarget {
    LiveTarget {
        device_id: "d1".into(),
        reg: LiveRegistration {
            live_key: Some(B64.encode(key().public_key().to_sec1_bytes())),
            ..reg
        },
        push_token: Some("ab".into()),
        push_env: Some(PushEnv::Sandbox),
        push_provider: PushProvider::Apns,
    }
}

fn calm() -> Board {
    Board {
        version: 1,
        at: 1000,
        running: vec!["api".into()],
        idle: 1,
        ..Board::default()
    }
}

fn needs() -> Board {
    Board {
        waiting: vec![Waiting {
            name: "checkout".into(),
            space: None,
            since: 990,
            error: false,
        }],
        ..calm()
    }
}

#[test]
fn a_running_card_gets_each_change_sealed_to_its_device() {
    let t = iphone(LiveRegistration {
        activity_token: Some("act".into()),
        activity_started_at: Some(900),
        ..Default::default()
    });
    let Plan::Send { token, live } = plan(&t, &needs(), Some(&(calm(), 990)), None, 1000) else {
        panic!("a change is sent");
    };
    assert_eq!((token.as_str(), live.event.as_str()), ("act", "update"));
    assert!(live.urgent && live.alert);
    assert_eq!(live.stale_at, 1000 + STALE_AFTER_SECS);
    let plain = open(&key(), live.blob.as_deref().unwrap()).unwrap();
    assert_eq!(serde_json::from_slice::<Board>(&plain).unwrap(), needs());
    assert!(live.valid());

    // Nothing new: nothing sent, until the refresh before it goes stale.
    assert_eq!(
        plan(&t, &calm(), Some(&(calm(), 990)), None, 1000),
        Plan::Skip
    );
    let later = 990 + REFRESH_AFTER_SECS;
    let Plan::Send { live, .. } = plan(&t, &calm(), Some(&(calm(), 990)), None, later) else {
        panic!("refreshed before it goes stale");
    };
    assert!(!live.urgent && !live.alert);
    assert_eq!(
        plan(
            &t,
            &Board::default(),
            Some(&(Board::default(), 990)),
            None,
            later
        ),
        Plan::Skip,
        "an empty board isn't kept alive"
    );
}

#[test]
fn a_new_card_starts_only_for_something_that_needs_the_owner() {
    let t = iphone(LiveRegistration {
        start_token: Some("start".into()),
        ..Default::default()
    });
    assert_eq!(plan(&t, &calm(), None, None, 1000), Plan::Skip);
    let Plan::Send { token, live } = plan(&t, &needs(), None, None, 1000) else {
        panic!("needs-you starts a card");
    };
    assert_eq!((token.as_str(), live.event.as_str()), ("start", "start"));

    // Started but its token not yet known: not started twice.
    let started = iphone(LiveRegistration {
        start_token: Some("start".into()),
        activity_started_at: Some(1000),
        ..Default::default()
    });
    assert_eq!(plan(&started, &needs(), None, None, 1100), Plan::Skip);

    // An eight-hour-old card is done: a new one starts.
    let old = iphone(LiveRegistration {
        start_token: Some("start".into()),
        activity_token: Some("act".into()),
        activity_started_at: Some(0),
        ..Default::default()
    });
    let Plan::Send { live, .. } = plan(&old, &needs(), None, None, MAX_CARD_SECS) else {
        panic!("an old card is replaced");
    };
    assert_eq!(live.event, "start");
}

#[test]
fn android_is_woken_without_a_blob_and_a_device_without_a_key_gets_nothing() {
    let android = LiveTarget {
        device_id: "a1".into(),
        reg: LiveRegistration::default(),
        push_token: Some("fcm".into()),
        push_env: None,
        push_provider: PushProvider::Fcm,
    };
    let Plan::Send { token, live } = plan(&android, &needs(), None, None, 1000) else {
        panic!("android is woken");
    };
    assert_eq!((token.as_str(), live.blob), ("fcm", None));
    let mut keyless = iphone(LiveRegistration {
        activity_token: Some("act".into()),
        ..Default::default()
    });
    keyless.reg.live_key = None;
    assert_eq!(plan(&keyless, &needs(), None, None, 1000), Plan::Skip);
}

#[test]
fn the_cards_payload_carries_only_ciphertext_and_generic_text() {
    let live = LivePush {
        event: "update".into(),
        blob: Some("c2VhbGVk".into()),
        urgent: true,
        alert: true,
        timestamp: 5,
        stale_at: 10,
    };
    let p = live_payload(&live);
    assert_eq!(
        p["aps"]["content-state"],
        serde_json::json!({ "v": 1, "b": "c2VhbGVk" })
    );
    assert_eq!(p["aps"]["alert"]["body"], "An agent needs you");
    assert_eq!(p["aps"]["stale-date"], 10);
    let quiet = live_payload(&LivePush {
        alert: false,
        ..live.clone()
    });
    assert!(quiet["aps"].get("alert").is_none());
    let start = live_payload(&LivePush {
        event: "start".into(),
        alert: false,
        ..live.clone()
    });
    assert_eq!(start["aps"]["attributes-type"], "AgentsActivity");
    assert!(
        start["aps"].get("alert").is_some(),
        "iOS needs an alert to start a card"
    );

    assert!(
        !LivePush {
            event: "stop".into(),
            ..live.clone()
        }
        .valid()
    );
    assert!(
        !LivePush {
            blob: Some("not base64url!".into()),
            ..live.clone()
        }
        .valid()
    );
    assert!(
        !LivePush {
            blob: Some("a".repeat(crate::live::MAX_BLOB_CHARS + 1)),
            ..live
        }
        .valid()
    );
}

#[test]
fn a_card_ends_after_fifteen_quiet_minutes_and_only_once() {
    let t = iphone(LiveRegistration {
        activity_token: Some("act".into()),
        activity_started_at: Some(900),
        ..Default::default()
    });
    let quiet_board = Board {
        version: 1,
        at: 1000,
        idle: 9,
        ..Board::default()
    };
    assert!(quiet_board.is_quiet() && !calm().is_quiet());
    let since = Quiet {
        since: 1000,
        ended: false,
    };
    assert_eq!(
        plan(
            &t,
            &quiet_board,
            Some(&(quiet_board.clone(), 1000)),
            Some(&since),
            1000 + QUIET_END_SECS - 1
        ),
        Plan::Skip,
        "still inside the fifteen minutes, and nothing changed"
    );
    let Plan::Send { token, live } = plan(
        &t,
        &quiet_board,
        Some(&(quiet_board.clone(), 1000)),
        Some(&since),
        1000 + QUIET_END_SECS,
    ) else {
        panic!("the card ends");
    };
    assert_eq!((token.as_str(), live.event.as_str()), ("act", "end"));
    assert_eq!(
        live_payload(&live)["aps"]["dismissal-date"],
        1000 + QUIET_END_SECS
    );
    let ended = Quiet {
        ended: true,
        ..since
    };
    assert_eq!(
        plan(
            &t,
            &quiet_board,
            None,
            Some(&ended),
            1000 + QUIET_END_SECS * 2
        ),
        Plan::Skip
    );
    // An all-idle board never starts a card.
    let start_only = iphone(LiveRegistration {
        start_token: Some("s".into()),
        ..Default::default()
    });
    assert_eq!(
        plan(&start_only, &quiet_board, None, None, 1000),
        Plan::Skip
    );
}

#[test]
fn always_keeps_the_card_and_renews_it_before_eight_hours() {
    let quiet_board = Board {
        version: 1,
        at: 1000,
        idle: 9,
        ..Board::default()
    };
    let always = |reg: LiveRegistration| {
        iphone(LiveRegistration {
            always: true,
            ..reg
        })
    };
    // Quiet for an hour: still no end.
    let t = always(LiveRegistration {
        activity_token: Some("act".into()),
        start_token: Some("s".into()),
        activity_started_at: Some(0),
        ..Default::default()
    });
    let long_quiet = Quiet {
        since: 0,
        ended: false,
    };
    assert_eq!(
        plan(
            &t,
            &quiet_board,
            Some(&(quiet_board.clone(), 3500)),
            Some(&long_quiet),
            3600
        ),
        Plan::Skip
    );
    // Seven and three quarter hours in: a fresh card through push-to-start, without a sound.
    let Plan::Send { token, live } = plan(
        &t,
        &quiet_board,
        Some(&(quiet_board.clone(), 1000)),
        None,
        RENEW_AFTER_SECS,
    ) else {
        panic!("renewed");
    };
    assert_eq!(
        (token.as_str(), live.event.as_str(), live.urgent),
        ("s", "start", false)
    );
    let p = live_payload(&live);
    assert!(p["aps"].get("alert").is_some() && p["aps"].get("sound").is_none());
    // No card at all, and nothing changed: always starts one.
    let none = always(LiveRegistration {
        start_token: Some("s".into()),
        ..Default::default()
    });
    let Plan::Send { live, .. } = plan(
        &none,
        &quiet_board,
        Some(&(quiet_board.clone(), 990)),
        None,
        1000,
    ) else {
        panic!("started");
    };
    assert_eq!(live.event, "start");
}
