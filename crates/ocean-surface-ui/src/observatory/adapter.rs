use futures_util::StreamExt;
use gloo_net::eventsource::futures::EventSource;
use gloo_net::http::Request;
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use super::domain::{EventEnvelope, IntegrityState, ObservatorySnapshot, ObservatoryState};
use super::reducer::{apply, from_snapshot_preserving_slots};
use super::replay::{
    apply_fetch_error, classify_http_error, finish_replay, fold_replay_page, replay_base,
    replay_path, replay_start, FetchError, ReplayPage, MAX_REPLAY_PAGES,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConnectionState {
    #[default]
    Connecting,
    Live,
    Resyncing,
    Offline,
    /// The live stream is paused while the rail shows a past cursor.
    Replay,
}

impl ConnectionState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::Live => "live",
            Self::Resyncing => "resyncing",
            Self::Offline => "offline",
            Self::Replay => "replay",
        }
    }
}

#[derive(Clone, Copy)]
pub struct ObservatoryClient {
    pub state: RwSignal<ObservatoryState>,
    pub connection: RwSignal<ConnectionState>,
    pub loading: RwSignal<bool>,
    generation: RwSignal<u64>,
}

impl ObservatoryClient {
    pub fn new() -> Self {
        Self {
            state: RwSignal::new(ObservatoryState::default()),
            connection: RwSignal::new(ConnectionState::Connecting),
            loading: RwSignal::new(true),
            generation: RwSignal::new(0),
        }
    }

    pub fn connect(self, base_url: RwSignal<String>) {
        let generation = self
            .generation
            .try_update(|generation| {
                *generation = generation.wrapping_add(1);
                *generation
            })
            .unwrap_or(0);
        spawn_local(async move {
            loop {
                if self.generation.get_untracked() != generation {
                    break;
                }
                self.connection.set(ConnectionState::Connecting);
                let base = base_url.get_untracked();
                if let Err(error) = self.fetch_snapshot(&base).await {
                    self.fail(error);
                    gloo_timers::future::TimeoutFuture::new(2_000).await;
                    continue;
                }
                let cursor = self.state.get_untracked().cursor;
                let stream_url = endpoint(&base, &format!("/v1/observatory/events?after={cursor}"));
                let mut source = match EventSource::new(&stream_url) {
                    Ok(source) => source,
                    Err(error) => {
                        self.fail(FetchError::Network(format!(
                            "event stream unavailable: {error}"
                        )));
                        gloo_timers::future::TimeoutFuture::new(2_000).await;
                        continue;
                    }
                };
                let message = match source.subscribe("message") {
                    Ok(stream) => stream,
                    Err(error) => {
                        self.fail(FetchError::Network(format!(
                            "event stream unavailable: {error}"
                        )));
                        gloo_timers::future::TimeoutFuture::new(2_000).await;
                        continue;
                    }
                };
                let reset = match source.subscribe("reset") {
                    Ok(stream) => stream,
                    Err(error) => {
                        self.fail(FetchError::Network(format!(
                            "reset stream unavailable: {error}"
                        )));
                        gloo_timers::future::TimeoutFuture::new(2_000).await;
                        continue;
                    }
                };
                self.connection.set(ConnectionState::Live);
                self.loading.set(false);
                let mut stream = futures_util::stream::select(message, reset);
                let mut needs_snapshot = false;
                while let Some(frame) = stream.next().await {
                    if self.generation.get_untracked() != generation {
                        source.close();
                        return;
                    }
                    let Ok((event_name, message)) = frame else {
                        break;
                    };
                    if event_name == "reset" {
                        needs_snapshot = true;
                        break;
                    }
                    let Some(data) = message.data().as_string() else {
                        continue;
                    };
                    match serde_json::from_str::<EventEnvelope>(&data) {
                        Ok(event) => {
                            self.state.update(|state| {
                                *state = apply(state.clone(), event);
                            });
                            if matches!(
                                self.state.get_untracked().integrity,
                                IntegrityState::Gap | IntegrityState::Stale
                            ) {
                                needs_snapshot = true;
                                break;
                            }
                        }
                        Err(error) => {
                            log::warn!("unparseable Observatory frame: {error}");
                        }
                    }
                }
                source.close();
                if needs_snapshot {
                    self.connection.set(ConnectionState::Resyncing);
                    let _ = self.fetch_snapshot(&base).await;
                } else {
                    self.connection.set(ConnectionState::Offline);
                    self.state.update(|state| {
                        state.integrity = IntegrityState::Disconnected;
                    });
                }
                gloo_timers::future::TimeoutFuture::new(1_200).await;
            }
        });
    }

    pub fn stop(self) {
        self.generation
            .update(|generation| *generation = generation.wrapping_add(1));
    }

    pub fn refresh(self, base_url: RwSignal<String>) {
        let base = base_url.get_untracked();
        spawn_local(async move {
            self.connection.set(ConnectionState::Resyncing);
            if let Err(error) = self.fetch_snapshot(&base).await {
                self.fail(error);
            } else {
                self.connection.set(ConnectionState::Live);
            }
        });
    }

    /// Show the floor as it was at `target`. The daemon snapshots only its
    /// current watermark (`409 snapshot_not_historical` otherwise), so the
    /// past is rebuilt by folding `/v1/observatory/replay` pages through
    /// `target` with the live reducer. Stops the live stream; a newer scrub,
    /// Live, or close cancels this one.
    pub fn replay_to(self, base_url: RwSignal<String>, target: u64) {
        let base = base_url.get_untracked();
        let generation = self
            .generation
            .try_update(|generation| {
                *generation = generation.wrapping_add(1);
                *generation
            })
            .unwrap_or(0);
        spawn_local(async move {
            self.loading.set(true);
            self.connection.set(ConnectionState::Replay);
            match self.fold_replay(&base, target, generation).await {
                Ok(Some(state)) => {
                    self.state.set(state);
                    self.loading.set(false);
                }
                Ok(None) => {}
                Err(error) => {
                    if self.generation.get_untracked() == generation {
                        self.fail(error);
                    }
                }
            }
        });
    }

    /// `Ok(None)` when superseded by a newer generation.
    async fn fold_replay(
        &self,
        base: &str,
        target: u64,
        generation: u64,
    ) -> Result<Option<ObservatoryState>, FetchError> {
        let previous = self.state.get_untracked();
        let mut state = replay_base(&previous);
        let mut after = replay_start(previous.earliest_cursor);
        let mut pages = 0;
        while after < target {
            if pages == MAX_REPLAY_PAGES {
                return Err(FetchError::ReplayUnavailable(format!(
                    "more than {MAX_REPLAY_PAGES} replay pages to reach cursor {target}"
                )));
            }
            pages += 1;
            let response = Request::get(&endpoint(base, &replay_path(after, target)))
                .send()
                .await
                .map_err(|error| FetchError::Network(format!("replay unavailable: {error}")))?;
            if !response.ok() {
                let status = response.status();
                let detail = response.text().await.unwrap_or_default();
                return Err(classify_http_error(status, &detail, true));
            }
            let page = response
                .json::<ReplayPage>()
                .await
                .map_err(|error| FetchError::Invalid(format!("replay page invalid: {error}")))?;
            if self.generation.get_untracked() != generation {
                return Ok(None);
            }
            state = fold_replay_page(state, &page, target);
            match page.next_after {
                Some(next) if page.has_more && !page.complete && next > after => after = next,
                _ => break,
            }
        }
        if self.generation.get_untracked() != generation {
            return Ok(None);
        }
        Ok(Some(finish_replay(state, target)))
    }

    async fn fetch_snapshot(&self, base: &str) -> Result<(), FetchError> {
        self.loading.set(true);
        let response = Request::get(&endpoint(base, "/v1/observatory/snapshot"))
            .send()
            .await
            .map_err(|error| FetchError::Network(format!("snapshot unavailable: {error}")))?;
        if !response.ok() {
            let status = response.status();
            let detail = response.text().await.unwrap_or_default();
            return Err(classify_http_error(status, &detail, false));
        }
        let snapshot = response
            .json::<ObservatorySnapshot>()
            .await
            .map_err(|error| FetchError::Invalid(format!("snapshot invalid: {error}")))?;
        // Preserve the session-local slot registry across refreshes, resyncs,
        // and replay scrubbing so existing cubicles never move.
        let previous = self.state.get_untracked();
        self.state
            .set(from_snapshot_preserving_slots(snapshot, &previous));
        self.loading.set(false);
        Ok(())
    }

    /// Record a failed fetch. Only a genuine connection loss reads as
    /// offline; `409 snapshot_not_historical` and other replay-range answers
    /// mean history is unavailable while the daemon stays reachable.
    fn fail(&self, error: FetchError) {
        self.loading.set(false);
        let mut disconnected = true;
        self.state.update(|state| {
            disconnected = apply_fetch_error(state, &error);
        });
        self.connection.set(if disconnected {
            ConnectionState::Offline
        } else {
            ConnectionState::Replay
        });
    }
}

fn endpoint(base: &str, path: &str) -> String {
    if base.trim().is_empty() {
        path.to_owned()
    } else {
        format!("{}{}", base.trim_end_matches('/'), path)
    }
}

#[cfg(test)]
mod tests {
    use super::endpoint;

    #[test]
    fn endpoint_preserves_same_origin_and_direct_daemon_paths() {
        assert_eq!(
            endpoint("", "/v1/observatory/snapshot"),
            "/v1/observatory/snapshot"
        );
        assert_eq!(
            endpoint("http://127.0.0.1:4780/", "/v1/observatory/snapshot"),
            "http://127.0.0.1:4780/v1/observatory/snapshot"
        );
    }
}
