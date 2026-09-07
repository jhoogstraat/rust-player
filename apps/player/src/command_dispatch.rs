use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use player_core::{Command, Runtime};

/// One FIFO worker for every window's commands. The sender is cheap to clone;
/// the runtime call stays off the GPUI thread and remains strictly ordered.
pub(crate) struct CommandDispatcher {
    sender: Mutex<Option<Sender<Command>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl CommandDispatcher {
    pub(crate) fn new(runtime: Arc<dyn Runtime>) -> Self {
        Self::with_handler(move |command| runtime.command(command).is_some())
    }

    fn with_handler(handler: impl Fn(Command) -> bool + Send + 'static) -> Self {
        let (sender, receiver) = mpsc::channel::<Command>();
        let worker = thread::Builder::new()
            .name("player-command-dispatcher".into())
            .spawn(move || {
                for command in receiver {
                    if !handler(command.clone()) {
                        log::warn!("[ui] runtime rejected {command:?}");
                    }
                }
            })
            .expect("failed to start command dispatcher");
        Self {
            sender: Mutex::new(Some(sender)),
            worker: Mutex::new(Some(worker)),
        }
    }

    pub(crate) fn send(&self, command: Command) {
        let accepted = self
            .sender
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|sender| sender.send(command.clone()).is_ok());
        if !accepted {
            log::warn!("[ui] command dispatcher rejected {command:?}");
        }
    }

    pub(crate) fn shutdown(&self) {
        self.sender.lock().unwrap().take();
        if let Some(worker) = self.worker.lock().unwrap().take() {
            worker.join().expect("command dispatcher panicked");
        }
    }
}

impl Drop for CommandDispatcher {
    fn drop(&mut self) {
        self.sender.get_mut().unwrap().take();
        if let Some(worker) = self.worker.get_mut().unwrap().take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn enqueue_is_nonblocking_and_fifo() {
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let dispatcher = CommandDispatcher::with_handler({
            let seen = seen.clone();
            move |command| {
                if matches!(command, Command::Next) {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                }
                seen.lock().unwrap().push(format!("{command:?}"));
                true
            }
        });

        dispatcher.send(Command::Next);
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let (sent_tx, sent_rx) = mpsc::channel();
        let sent_while_blocked = std::thread::scope(|scope| {
            scope.spawn(|| {
                dispatcher.send(Command::Previous);
                sent_tx.send(()).unwrap();
            });
            let sent = sent_rx.recv_timeout(Duration::from_secs(1));
            // Release even on failure so a regression cannot deadlock cleanup.
            release_tx.send(()).unwrap();
            sent
        });
        dispatcher.shutdown();
        sent_while_blocked.unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.as_slice(), ["Next", "Previous"]);
    }
}
