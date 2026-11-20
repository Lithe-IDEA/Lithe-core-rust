//! Public AskPass decision API. Channel behaviour is covered by the unit tests
//! next to the transport and by the platform tests that run the real helper.
use lithe_git_host::authentication::{respond, Confirmation};

#[test]
fn retry_decisions_and_stale_responses_are_bounded() {
    let confirmation = Confirmation::new().unwrap();
    assert!(respond(&confirmation.request_id, Some("retry".into())));
    assert!(confirmation.wait(|| false).unwrap());
    let confirmation = Confirmation::new().unwrap();
    assert!(respond(&confirmation.request_id, Some("skip".into())));
    assert!(!confirmation.wait(|| false).unwrap());
    let confirmation = Confirmation::new().unwrap();
    let id = confirmation.request_id.clone();
    assert!(!confirmation.wait(|| true).unwrap());
    drop(confirmation);
    assert!(!respond(&id, Some("retry".into())));
}

#[test]
fn unknown_requests_are_refused() {
    assert!(!respond("not-a-registered-request", None));
    assert!(!respond("not-a-registered-request", Some("answer".into())));
}
