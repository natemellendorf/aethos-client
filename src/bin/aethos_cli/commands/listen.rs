use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use clap::Args;
use serde_json::json;

const POLL_INTERVAL: Duration = Duration::from_millis(500);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
const DEFAULT_ENCOUNTER_WINDOW: Duration = Duration::from_secs(3);
const EXIT_SUCCESS: u8 = 0;
const EXIT_TIMEOUT_NO_MESSAGES: u8 = 2;

static SIGNAL_RECEIVED: AtomicBool = AtomicBool::new(false);
static REQUESTED_EXIT_CODE: AtomicU8 = AtomicU8::new(EXIT_SUCCESS);

#[derive(Debug, Args, Clone)]
/// Connect to relay and stream incoming messages in real time. Exits on SIGINT or --timeout.
/// Exit code 2 if timeout fires with zero messages received; exit code 0 otherwise.
pub struct ListenArgs {
    #[arg(
        long,
        help = "Max seconds to listen (exit code 2 if timeout with no messages)"
    )]
    pub timeout: Option<u64>,

    #[arg(long, help = "Override relay endpoint for this session")]
    pub relay: Option<String>,

    #[arg(
        long = "filter-from",
        help = "Only show messages from this wayfarer_id"
    )]
    pub filter_from: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShutdownReason {
    Signal,
    Timeout,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ListenOutcome {
    reason: ShutdownReason,
    messages_received: usize,
    exit_code: u8,
}

pub fn run(args: &ListenArgs, state: &crate::state::CliState) -> Result<(), String> {
    REQUESTED_EXIT_CODE.store(EXIT_SUCCESS, Ordering::Relaxed);
    SIGNAL_RECEIVED.store(false, Ordering::Relaxed);
    let shutdown = Arc::new(AtomicBool::new(false));
    let outcome =
        execute_with_runtime::<SystemRuntime, _>(args, state, &shutdown, |event, data| {
            crate::output::emit_event(event, data)
        });

    match outcome {
        Ok(outcome) => {
            REQUESTED_EXIT_CODE.store(outcome.exit_code, Ordering::Relaxed);
            Ok(())
        }
        Err(err) => {
            crate::output::emit_error(&err);
            Err(err)
        }
    }
}

pub fn take_requested_exit_code() -> u8 {
    REQUESTED_EXIT_CODE.swap(EXIT_SUCCESS, Ordering::Relaxed)
}

trait ListenRuntime {
    type Session;
    type Moment: Copy;

    fn install_signal_handler() -> Result<(), String>;
    fn load_identity() -> Result<crate::aethos_core::identity_store::LocalIdentitySummary, String>;
    fn open_session(
        relay_ws: &str,
        identity: &crate::aethos_core::identity_store::LocalIdentitySummary,
    ) -> Result<Self::Session, String>;
    fn poll(
        session: &mut Self::Session,
        identity: &crate::aethos_core::identity_store::LocalIdentitySummary,
        encounter_window: Duration,
    ) -> Result<crate::relay::client::EncounterReport, String>;
    fn maybe_send_heartbeat(session: &mut Self::Session) -> Result<bool, String>;
    fn close_session(session: Self::Session, reason: &str);
    fn now() -> Self::Moment;
    fn elapsed(since: Self::Moment) -> Duration;
    fn sleep(duration: Duration);
}

struct SystemRuntime;

impl ListenRuntime for SystemRuntime {
    type Session = crate::relay::client::RelayPersistentSession;
    type Moment = Instant;

    fn install_signal_handler() -> Result<(), String> {
        install_sigint_handler();
        Ok(())
    }

    fn load_identity() -> Result<crate::aethos_core::identity_store::LocalIdentitySummary, String> {
        crate::aethos_core::identity_store::ensure_local_identity()
    }

    fn open_session(
        relay_ws: &str,
        identity: &crate::aethos_core::identity_store::LocalIdentitySummary,
    ) -> Result<Self::Session, String> {
        crate::relay::client::open_relay_persistent_session(relay_ws, identity, None)
    }

    fn poll(
        session: &mut Self::Session,
        identity: &crate::aethos_core::identity_store::LocalIdentitySummary,
        encounter_window: Duration,
    ) -> Result<crate::relay::client::EncounterReport, String> {
        crate::relay::client::poll_relay_inbound_on_persistent_session(
            session,
            identity,
            None,
            encounter_window,
        )
    }

    fn maybe_send_heartbeat(session: &mut Self::Session) -> Result<bool, String> {
        crate::relay::client::maybe_send_relay_heartbeat(session)
    }

    fn close_session(session: Self::Session, reason: &str) {
        crate::relay::client::close_relay_persistent_session(session, reason);
    }

    fn now() -> Self::Moment {
        Instant::now()
    }

    fn elapsed(since: Self::Moment) -> Duration {
        since.elapsed()
    }

    fn sleep(duration: Duration) {
        thread::sleep(duration);
    }
}

fn execute_with_runtime<R, F>(
    args: &ListenArgs,
    state: &crate::state::CliState,
    shutdown: &Arc<AtomicBool>,
    mut emit: F,
) -> Result<ListenOutcome, String>
where
    R: ListenRuntime,
    F: FnMut(&str, serde_json::Value),
{
    R::install_signal_handler()?;

    let identity = R::load_identity()?;
    let relay_input = args.relay.as_deref().unwrap_or(&state.relay_endpoint);
    let relay_ws = crate::relay::client::to_ws_endpoint(relay_input);
    let mut session = R::open_session(&relay_ws, &identity)
        .map_err(|err| format!("listen connect failed: {err}"))?;

    emit(
        "listen_started",
        json!({
            "relay": relay_ws,
            "wayfarer_id": identity.wayfarer_id,
        }),
    );

    let started_at = R::now();
    let mut last_heartbeat_event_at = started_at;
    let timeout = args.timeout.map(Duration::from_secs);
    let filter_from = args.filter_from.as_deref();
    let mut messages_received = 0usize;

    loop {
        mirror_signal_into_shutdown(shutdown);
        if shutdown.load(Ordering::Relaxed) {
            R::close_session(session, "signal");
            emit_shutdown(&mut emit, ShutdownReason::Signal, messages_received);
            return Ok(ListenOutcome {
                reason: ShutdownReason::Signal,
                messages_received,
                exit_code: EXIT_SUCCESS,
            });
        }

        if timeout_reached::<R>(started_at, timeout) {
            R::close_session(session, "timeout");
            emit_shutdown(&mut emit, ShutdownReason::Timeout, messages_received);
            return Ok(timeout_outcome(messages_received));
        }

        let encounter_window = encounter_window::<R>(started_at, timeout);
        let report = match R::poll(&mut session, &identity, encounter_window) {
            Ok(report) => report,
            Err(err) => {
                R::close_session(session, "poll_error");
                return Err(format!("listen poll failed: {err}"));
            }
        };

        let matched_count = matched_message_count(&report, filter_from);
        if matched_count > 0 {
            messages_received = messages_received.saturating_add(matched_count);
            emit(
                "message_received",
                json!({
                    "from": serde_json::Value::Null,
                    "count": matched_count,
                }),
            );
        }

        if let Err(err) = R::maybe_send_heartbeat(&mut session) {
            R::close_session(session, "heartbeat_error");
            return Err(format!("listen heartbeat failed: {err}"));
        }

        if R::elapsed(last_heartbeat_event_at) >= HEARTBEAT_INTERVAL {
            emit(
                "heartbeat",
                json!({
                    "uptime_ms": R::elapsed(started_at).as_millis() as u64,
                    "messages_received": messages_received,
                }),
            );
            last_heartbeat_event_at = R::now();
        }

        mirror_signal_into_shutdown(shutdown);
        if shutdown.load(Ordering::Relaxed) {
            R::close_session(session, "signal");
            emit_shutdown(&mut emit, ShutdownReason::Signal, messages_received);
            return Ok(ListenOutcome {
                reason: ShutdownReason::Signal,
                messages_received,
                exit_code: EXIT_SUCCESS,
            });
        }

        if timeout_reached::<R>(started_at, timeout) {
            R::close_session(session, "timeout");
            emit_shutdown(&mut emit, ShutdownReason::Timeout, messages_received);
            return Ok(timeout_outcome(messages_received));
        }

        R::sleep(POLL_INTERVAL);
    }
}

fn matched_message_count(
    report: &crate::relay::client::EncounterReport,
    filter_from: Option<&str>,
) -> usize {
    match filter_from {
        Some(filter) => report
            .pulled_messages
            .iter()
            .filter(|message| message.author_wayfarer_id.as_deref() == Some(filter))
            .count(),
        None => report.transferred_items,
    }
}

fn timeout_reached<R: ListenRuntime>(started_at: R::Moment, timeout: Option<Duration>) -> bool {
    timeout.is_some_and(|limit| R::elapsed(started_at) >= limit)
}

fn encounter_window<R: ListenRuntime>(
    started_at: R::Moment,
    timeout: Option<Duration>,
) -> Duration {
    match timeout {
        Some(limit) => limit
            .checked_sub(R::elapsed(started_at))
            .unwrap_or(Duration::from_millis(1))
            .min(DEFAULT_ENCOUNTER_WINDOW)
            .max(Duration::from_millis(1)),
        None => DEFAULT_ENCOUNTER_WINDOW,
    }
}

fn emit_shutdown<F>(emit: &mut F, reason: ShutdownReason, messages_received: usize)
where
    F: FnMut(&str, serde_json::Value),
{
    emit(
        "shutdown",
        json!({
            "reason": shutdown_reason_name(reason),
            "messages_received": messages_received,
        }),
    );
}

fn shutdown_reason_name(reason: ShutdownReason) -> &'static str {
    match reason {
        ShutdownReason::Signal => "signal",
        ShutdownReason::Timeout => "timeout",
    }
}

fn timeout_outcome(messages_received: usize) -> ListenOutcome {
    ListenOutcome {
        reason: ShutdownReason::Timeout,
        messages_received,
        exit_code: if messages_received == 0 {
            EXIT_TIMEOUT_NO_MESSAGES
        } else {
            EXIT_SUCCESS
        },
    }
}

fn mirror_signal_into_shutdown(shutdown: &Arc<AtomicBool>) {
    if SIGNAL_RECEIVED.load(Ordering::Relaxed) {
        shutdown.store(true, Ordering::Relaxed);
    }
}

extern "C" fn handle_sigint(_: i32) {
    SIGNAL_RECEIVED.store(true, Ordering::Relaxed);
}

fn install_sigint_handler() {
    unsafe {
        signal(SIGINT, handle_sigint);
    }
}

const SIGINT: i32 = 2;

unsafe extern "C" {
    fn signal(sig: i32, handler: extern "C" fn(i32)) -> usize;
}

#[cfg(test)]
mod tests {
    use super::{execute_with_runtime, ListenArgs, ListenOutcome, ListenRuntime, ShutdownReason};
    use clap::Parser;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::Duration;

    fn reset_runtime() {
        MockRuntime::set_time_ms(0);
        MockRuntime::set_reports(Vec::new());
        MockRuntime::set_open_error(None);
        MockRuntime::set_closed_reason(None);
    }

    #[derive(Parser)]
    struct ListenCli {
        #[command(flatten)]
        args: ListenArgs,
    }

    #[test]
    fn parses_listen_args() {
        let cli = ListenCli::try_parse_from([
            "aethos-cli",
            "--timeout",
            "9",
            "--relay",
            "localhost:8082",
            "--filter-from",
            "abc123",
        ])
        .expect("parse listen args");

        assert_eq!(cli.args.timeout, Some(9));
        assert_eq!(cli.args.relay.as_deref(), Some("localhost:8082"));
        assert_eq!(cli.args.filter_from.as_deref(), Some("abc123"));
    }

    #[test]
    fn timeout_without_messages_returns_exit_code_two() {
        let _guard = crate::global_test_env_lock().lock().expect("lock env");
        reset_runtime();

        let state = crate::state::CliState::from_cli_args(None, None, false);

        MockRuntime::set_reports(vec![Ok(report_with(0, &[]))]);

        let args = ListenArgs {
            timeout: Some(1),
            relay: Some("localhost:8082".to_string()),
            filter_from: None,
        };
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut events = Vec::new();

        let outcome =
            execute_with_runtime::<MockRuntime, _>(&args, &state, &shutdown, |event, data| {
                events.push((event.to_string(), data));
            })
            .expect("listen run succeeds");

        assert_eq!(outcome, timeout_outcome_for_assert(0));
        assert_eq!(events[0].0, "listen_started");
        assert_eq!(events.last().expect("shutdown event").0, "shutdown");
        assert_eq!(
            events.last().expect("shutdown event").1["reason"],
            "timeout"
        );
        assert_eq!(
            events.last().expect("shutdown event").1["messages_received"],
            0
        );
    }

    #[test]
    fn timeout_after_messages_returns_exit_code_zero() {
        let _guard = crate::global_test_env_lock().lock().expect("lock env");
        reset_runtime();

        let state = crate::state::CliState::from_cli_args(None, None, false);

        MockRuntime::set_reports(vec![Ok(report_with(
            1,
            &[Some(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            )],
        ))]);

        let args = ListenArgs {
            timeout: Some(1),
            relay: Some("localhost:8082".to_string()),
            filter_from: None,
        };
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let outcome =
            execute_with_runtime::<MockRuntime, _>(&args, &state, &shutdown, |_event, _data| {})
                .expect("listen run succeeds");

        assert_eq!(
            outcome,
            ListenOutcome {
                reason: ShutdownReason::Timeout,
                messages_received: 1,
                exit_code: 0,
            }
        );
    }

    fn timeout_outcome_for_assert(messages_received: usize) -> ListenOutcome {
        ListenOutcome {
            reason: ShutdownReason::Timeout,
            messages_received,
            exit_code: if messages_received == 0 { 2 } else { 0 },
        }
    }

    fn report_with(
        transferred_items: usize,
        authors: &[Option<&str>],
    ) -> crate::relay::client::EncounterReport {
        crate::relay::client::EncounterReport {
            transferred_items,
            pulled_messages: authors
                .iter()
                .enumerate()
                .map(
                    |(index, author)| crate::relay::client::EncounterMessagePreview {
                        author_wayfarer_id: author.map(|value| value.to_string()),
                        session_peer: None,
                        transport_peer: None,
                        item_id: format!("item-{index}"),
                        body_bytes: Vec::new(),
                        text: String::new(),
                        received_at_unix: 0,
                        manifest_id_hex: None,
                    },
                )
                .collect(),
            trace_requested_by_peer: false,
            trace_receipted_by_peer: false,
            remote_closed: false,
        }
    }

    struct MockRuntime;

    impl MockRuntime {
        fn time_ms() -> &'static AtomicU64 {
            static VALUE: AtomicU64 = AtomicU64::new(0);
            &VALUE
        }

        fn reports(
        ) -> &'static Mutex<VecDeque<Result<crate::relay::client::EncounterReport, String>>>
        {
            static REPORTS: OnceLock<
                Mutex<VecDeque<Result<crate::relay::client::EncounterReport, String>>>,
            > = OnceLock::new();
            REPORTS.get_or_init(|| Mutex::new(VecDeque::new()))
        }

        fn open_error() -> &'static Mutex<Option<String>> {
            static OPEN_ERROR: OnceLock<Mutex<Option<String>>> = OnceLock::new();
            OPEN_ERROR.get_or_init(|| Mutex::new(None))
        }

        fn closed_reason() -> &'static Mutex<Option<String>> {
            static CLOSED_REASON: OnceLock<Mutex<Option<String>>> = OnceLock::new();
            CLOSED_REASON.get_or_init(|| Mutex::new(None))
        }

        fn set_time_ms(value: u64) {
            Self::time_ms().store(value, Ordering::Relaxed);
        }

        fn set_reports(reports: Vec<Result<crate::relay::client::EncounterReport, String>>) {
            let mut guard = Self::reports().lock().expect("lock reports");
            *guard = reports.into_iter().collect();
        }

        fn set_open_error(error: Option<&str>) {
            *Self::open_error().lock().expect("lock open error") = error.map(str::to_string);
        }

        fn set_closed_reason(reason: Option<&str>) {
            *Self::closed_reason().lock().expect("lock closed reason") = reason.map(str::to_string);
        }
    }

    impl ListenRuntime for MockRuntime {
        type Session = usize;
        type Moment = u64;

        fn install_signal_handler() -> Result<(), String> {
            Ok(())
        }

        fn load_identity(
        ) -> Result<crate::aethos_core::identity_store::LocalIdentitySummary, String> {
            Ok(crate::aethos_core::identity_store::LocalIdentitySummary {
                wayfarer_id: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                    .to_string(),
                device_id: "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210"
                    .to_string(),
                verifying_key_b64: "ZmFrZS12ZXJpZnlpbmcta2V5".to_string(),
                device_name: "test-device".to_string(),
            })
        }

        fn open_session(
            _relay_ws: &str,
            _identity: &crate::aethos_core::identity_store::LocalIdentitySummary,
        ) -> Result<Self::Session, String> {
            if let Some(err) = Self::open_error().lock().expect("lock open error").clone() {
                return Err(err);
            }
            Ok(1)
        }

        fn poll(
            _session: &mut Self::Session,
            _identity: &crate::aethos_core::identity_store::LocalIdentitySummary,
            _encounter_window: Duration,
        ) -> Result<crate::relay::client::EncounterReport, String> {
            Self::reports()
                .lock()
                .expect("lock reports")
                .pop_front()
                .unwrap_or_else(|| Ok(report_with(0, &[])))
        }

        fn maybe_send_heartbeat(_session: &mut Self::Session) -> Result<bool, String> {
            Ok(false)
        }

        fn close_session(_session: Self::Session, reason: &str) {
            Self::set_closed_reason(Some(reason));
        }

        fn now() -> Self::Moment {
            Self::time_ms().load(Ordering::Relaxed)
        }

        fn elapsed(since: Self::Moment) -> Duration {
            Duration::from_millis(
                Self::time_ms()
                    .load(Ordering::Relaxed)
                    .saturating_sub(since),
            )
        }

        fn sleep(duration: Duration) {
            Self::time_ms().fetch_add(duration.as_millis() as u64, Ordering::Relaxed);
        }
    }
}
