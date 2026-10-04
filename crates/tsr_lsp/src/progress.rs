//! A bounded progress actor with delayed creation, ordered labels and refcounts.
use crate::{
    client::{self, Client},
    dynamic_queue::DynamicQueue,
};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;
use tsr_ipc::{Context, ContextError};
use tsr_lsproto as lsp;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Event {
    Start(String),
    Finish(String),
    Delay,
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum Output {
    Create(String),
    Begin(String, String),
    Report(String, String),
    End(String),
}

#[derive(Default)]
struct State {
    loading: Vec<(String, usize)>,
    next_token: u64,
    token: String,
    begun: bool,
    delay_fired: bool,
}
impl State {
    // port: tsc/internal/lsp/progress.go:projectLoadingProgress.run
    fn step(&mut self, event: Event, immediate: bool) -> Vec<Output> {
        let mut out = Vec::new();
        match event {
            Event::Start(text) => {
                if let Some((_, count)) = self.loading.iter_mut().find(|(name, _)| *name == text) {
                    *count += 1;
                } else {
                    self.loading.push((text.clone(), 1));
                }
                if self.token.is_empty() {
                    self.next_token += 1;
                    self.token = format!("tsgo-loading-{}", self.next_token);
                    self.begun = false;
                    self.delay_fired = immediate;
                    if immediate {
                        out.push(Output::Create(self.token.clone()));
                    }
                }
                if self.delay_fired {
                    self.begin_or_report(text, &mut out);
                }
            }
            Event::Finish(text) => {
                if let Some(at) = self.loading.iter().position(|(name, _)| *name == text) {
                    self.loading[at].1 -= 1;
                    if self.loading[at].1 == 0 {
                        self.loading.remove(at);
                    }
                }
                if !self.token.is_empty() {
                    if self.loading.is_empty() {
                        if self.begun {
                            out.push(Output::End(self.token.clone()));
                        }
                        self.token.clear();
                    } else if self.delay_fired {
                        out.push(Output::Report(
                            self.token.clone(),
                            self.loading[0].0.clone(),
                        ));
                    }
                }
            }
            Event::Delay => {
                self.delay_fired = true;
                if !self.token.is_empty() && !self.loading.is_empty() {
                    out.push(Output::Create(self.token.clone()));
                    self.begin_or_report(self.loading[0].0.clone(), &mut out);
                }
            }
        }
        out
    }
    // port: tsc/internal/lsp/progress.go:projectLoadingProgress.beginOrReport
    fn begin_or_report(&mut self, text: String, out: &mut Vec<Output>) {
        out.push(if self.begun {
            Output::Report(self.token.clone(), text)
        } else {
            Output::Begin(self.token.clone(), text)
        });
        self.begun = true;
    }
}

pub struct LoadingProgress {
    events: DynamicQueue<Event>,
    context: Context,
    worker: Option<JoinHandle<()>>,
}
impl LoadingProgress {
    // port: tsc/internal/lsp/progress.go:newProjectLoadingProgress
    pub fn new(
        context: &Context,
        client: Arc<dyn Client>,
        delay: Duration,
        loading_title: String,
    ) -> Self {
        let context = context.with_cancel();
        let events = DynamicQueue::bounded(64);
        let worker_events = events.clone();
        let worker_context = context.clone();
        let worker = std::thread::spawn(move || {
            let mut state = State::default();
            let mut timer: Option<Context> = None;
            loop {
                let event = match worker_events.get(timer.as_ref().unwrap_or(&worker_context)) {
                    Ok(event) => event,
                    Err(ContextError::DeadlineExceeded) if worker_context.err().is_none() => {
                        timer = None;
                        Event::Delay
                    }
                    Err(_) => break,
                };
                let was_empty = state.token.is_empty();
                for output in state.step(event, delay.is_zero()) {
                    publish(client.as_ref(), &loading_title, output);
                }
                if state.token.is_empty() {
                    if let Some(timer) = timer.take() {
                        timer.cancel();
                    }
                } else if was_empty && !delay.is_zero() {
                    timer = Some(worker_context.with_timeout(delay));
                }
            }
            if let Some(timer) = timer {
                timer.cancel();
            }
        });
        Self {
            context,
            events,
            worker: Some(worker),
        }
    }
    // port: tsc/internal/lsp/progress.go:projectLoadingProgress.start
    pub fn start(&self, localized_message: String) {
        let _ = self
            .events
            .put(&self.context, Event::Start(localized_message));
    }
    // port: tsc/internal/lsp/progress.go:projectLoadingProgress.finish
    pub fn finish(&self, localized_message: String) {
        let _ = self
            .events
            .put(&self.context, Event::Finish(localized_message));
    }
}
impl Drop for LoadingProgress {
    fn drop(&mut self) {
        self.context.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn publish(client: &dyn Client, title: &str, output: Output) {
    let (token, value) = match output {
        Output::Create(token) => {
            let params = lsp::WorkDoneProgressCreateParams {
                token: lsp::IntegerOrString {
                    string: Some(Box::new(token)),
                    ..Default::default()
                },
            };
            if let Ok(params) = client::raw(&params) {
                let _ = client.request_without_waiting("window/workDoneProgress/create", params);
            }
            return;
        }
        Output::Begin(token, message) => (
            token,
            lsp::WorkDoneProgressBeginOrReportOrEnd {
                begin: Some(Box::new(lsp::WorkDoneProgressBegin {
                    title: title.into(),
                    message: Some(Box::new(message)),
                    ..Default::default()
                })),
                ..Default::default()
            },
        ),
        Output::Report(token, message) => (
            token,
            lsp::WorkDoneProgressBeginOrReportOrEnd {
                report: Some(Box::new(lsp::WorkDoneProgressReport {
                    message: Some(Box::new(message)),
                    ..Default::default()
                })),
                ..Default::default()
            },
        ),
        Output::End(token) => (
            token,
            lsp::WorkDoneProgressBeginOrReportOrEnd {
                end: Some(Box::default()),
                ..Default::default()
            },
        ),
    };
    let _ = client::notify(
        client,
        "$/progress",
        &lsp::ProgressParams {
            token: lsp::IntegerOrString {
                string: Some(Box::new(token)),
                ..Default::default()
            },
            value,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actor_delays_ui_and_closes_without_waiting_for_create_reply() {
        let ctx = Context::background();
        let (send, receive) = std::sync::mpsc::channel();
        let client = crate::rpc_client::RpcClient::new(
            ctx.clone(),
            Arc::new(move |value| {
                send.send(value).unwrap();
                Ok(())
            }),
        );
        let progress =
            LoadingProgress::new(&ctx, client, Duration::from_millis(10), "Loading".into());
        progress.start("project A".into());
        let create = receive.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(String::from_utf8(create.0)
            .unwrap()
            .contains("window/workDoneProgress/create"));
        let begin = receive.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(String::from_utf8(begin.0).unwrap().contains("begin"));
        progress.finish("project A".into());
        let end = receive.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(String::from_utf8(end.0).unwrap().contains("end"));
        drop(progress);
    }
    fn token(n: u64) -> String {
        format!("tsgo-loading-{n}")
    }
    #[test]
    fn delayed_refcounted_progress_preserves_the_first_remaining_label() {
        let mut state = State::default();
        assert!(state.step(Event::Start("A".into()), false).is_empty());
        assert!(state.step(Event::Finish("A".into()), false).is_empty());
        assert!(state
            .step(Event::Finish("unknown".into()), false)
            .is_empty());
        assert!(state.step(Event::Start("A".into()), false).is_empty());
        assert!(state.step(Event::Start("B".into()), false).is_empty());
        assert_eq!(
            state.step(Event::Delay, false),
            [
                Output::Create(token(2)),
                Output::Begin(token(2), "A".into())
            ]
        );
        assert_eq!(
            state.step(Event::Start("B".into()), false),
            [Output::Report(token(2), "B".into())]
        );
        assert_eq!(
            state.step(Event::Finish("A".into()), false),
            [Output::Report(token(2), "B".into())]
        );
        assert_eq!(
            state.step(Event::Finish("B".into()), false),
            [Output::Report(token(2), "B".into())]
        );
        assert_eq!(
            state.step(Event::Finish("B".into()), false),
            [Output::End(token(2))]
        );
        assert_eq!(
            state.step(Event::Start("C".into()), true),
            [
                Output::Create(token(3)),
                Output::Begin(token(3), "C".into())
            ]
        );
        assert_eq!(
            state.step(Event::Finish("C".into()), true),
            [Output::End(token(3))]
        );
    }
}
