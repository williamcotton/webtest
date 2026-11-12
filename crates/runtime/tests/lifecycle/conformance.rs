use super::*;
use futures::FutureExt;
use webtest_observation::{ResourceEventKind, WaitEventKind};

// Vary completion, acquisition, cleanup, and deadline orderings with reproducible
// seeds. The assertions concern externally observable ownership, not scheduler internals.
#[tokio::test(start_paused = true)]
async fn seeded_nested_execution_finishes_every_owned_scope_resource_and_wait() {
    for seed in 0..32u64 {
        let case = async {
            let mut random = seed + 1;
            let mut next = || {
                random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
                (random >> 32) % 20
            };
            let state = Arc::new(LifecycleState::default());
            for name in ["left", "right"] {
                state
                    .page_delays
                    .lock()
                    .unwrap()
                    .insert(name.into(), Duration::from_millis(next()));
            }
            for id in 0..6 {
                state
                    .page_creation_delays
                    .lock()
                    .unwrap()
                    .insert(id, Duration::from_millis(next()));
                state
                    .context_close_delays
                    .lock()
                    .unwrap()
                    .insert(id, Duration::from_millis(next()));
            }
            if seed % 4 == 0 {
                state.context_close_failures.lock().unwrap().insert(0);
            }
            let deadline = if seed % 2 == 0 { 10 } else { 200 };
            let plan = compile_source(&format!(
                r#"test "owned" {{
                timeout {deadline}ms {{
                    parallel {{
                        race {{ browser {{ evaluate "left" }} browser {{ evaluate "right" }} }}
                        retry 2 backoff 1ms max 1ms {{ browser {{ expect 1 == 2 }} }}
                    }}
                }}
            }}"#
            ));
            let result = Runner::new(Arc::default())
                .run(&plan, &LifecycleHost(state.clone()))
                .await;
            assert_scopes_finish_once_after_children(&result.events);
            let mut resources = BTreeMap::new();
            let mut waits = BTreeMap::new();
            for (index, fact) in result.events.iter().enumerate() {
                match fact {
                    ExecutionEvent::Resource { event, .. } => {
                        let key = (
                            event.resource.key.resource_id,
                            event.resource.key.generation_id,
                        );
                        let history = resources.entry(key).or_insert_with(Vec::new);
                        history.push(event.kind);
                        if event.resource.owner_scope_id.is_some()
                            && matches!(
                                event.kind,
                                ResourceEventKind::Released | ResourceEventKind::ReleaseFailed
                            )
                        {
                            assert!(result.events[index + 1..].iter().any(|fact|
                                matches!(fact, ExecutionEvent::Scope { event: scope, .. }
                                    if Some(scope.execution_context.scope_id) == event.resource.owner_scope_id && scope.outcome.is_some())));
                        }
                    }
                    ExecutionEvent::Wait { event, .. } => {
                        waits
                            .entry(event.registration_id)
                            .or_insert_with(Vec::new)
                            .push(event.kind);
                    }
                    _ => {}
                }
            }
            assert!(!resources.is_empty());
            for history in resources.values() {
                assert_eq!(history.first(), Some(&ResourceEventKind::AcquireStarted));
                assert_eq!(
                    history
                        .iter()
                        .filter(|kind| matches!(
                            kind,
                            ResourceEventKind::Released | ResourceEventKind::ReleaseFailed
                        ))
                        .count(),
                    1
                );
                assert!(matches!(
                    history.last(),
                    Some(ResourceEventKind::Released | ResourceEventKind::ReleaseFailed)
                ));
            }
            for history in waits.values() {
                assert!(matches!(
                    history.as_slice(),
                    [
                        WaitEventKind::Registered,
                        WaitEventKind::Ready | WaitEventKind::Cancelled
                    ]
                ));
            }
            let log = state.log();
            for id in 0..state.next_context.load(Ordering::SeqCst) {
                assert_eq!(
                    log.iter()
                        .filter(|entry| **entry == format!("context_close:{id}"))
                        .count(),
                    1
                );
            }
            for id in 0..state.next_session.load(Ordering::SeqCst) {
                assert_eq!(
                    log.iter()
                        .filter(|entry| **entry == format!("session_close:{id}"))
                        .count(),
                    1
                );
            }
            tokio::time::advance(Duration::from_secs(60)).await;
            tokio::task::yield_now().await;
            assert_eq!(state.log(), log, "work survived its parent");
        };
        assert!(
            std::panic::AssertUnwindSafe(case)
                .catch_unwind()
                .await
                .is_ok(),
            "ownership conformance seed={seed}"
        );
    }
}
