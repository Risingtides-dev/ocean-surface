## Review
- Correct: The stale-generation guard now runs immediately after `rehydrate_transcript(...).await` and breaks before `EventSource::new` or slot replacement (`crates/ocean-surface-ui/src/daemon.rs:1836-1877`). No await or event-loop yield exists between the guard and installation, so the identified race is closed.
- **Clean: no blocker remains. Review gate passed.**