use std::fmt::Display;

use async_trait::async_trait;
use irc::client::Client;
use irc::proto::{Command, Message};
use psutil::process::Process;
use tokio::runtime::Handle;

use crate::Error as ZetaError;

use super::{Author, Name, Plugin, Version};

pub struct Health;

/// Process telemetry snapshot.
pub struct Snapshot {
    /// The RSS memory usage, as MiB.
    pub rss_mib: f64,
    /// The VMS memory usage, as MiB.
    pub vms_mib: f64,
    /// The shared memory usage, as MiB.
    pub shared_mib: f64,
    /// The number of tasks currently scheduled in the runtime's global queue.
    pub global_queue_depth: usize,
    /// The current number of alive tasks in the runtime.
    pub num_alive_tasks: usize,
    /// The number of worker threads used by the runtime.
    pub num_workers: usize,
}

#[async_trait]
impl Plugin for Health {
    fn new() -> Health {
        Health
    }

    fn name() -> Name {
        Name("health")
    }

    fn author() -> Author {
        Author("Mikkel Kroman <mk@maero.dk>")
    }

    fn version() -> Version {
        Version("0.1")
    }

    async fn handle_message(&self, message: &Message, client: &Client) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref message) = message.command
            && message.starts_with(".health")
            && let Some(snapshot) = Snapshot::capture()
        {
            client.send_privmsg(
                channel,
                format!("\x0310>\x0f\x02 Health\x02\x0310: {snapshot}"),
            )?;
        }

        Ok(())
    }
}

impl Snapshot {
    #[allow(clippy::cast_precision_loss)]
    pub fn capture() -> Option<Snapshot> {
        if let Ok(proc) = Process::current()
            && let Ok(memory) = proc.memory_info()
        {
            // Capture memory information
            let rss_mib = memory.rss() as f64 / 1024. / 1024.;
            let vms_mib = memory.vms() as f64 / 1024. / 1024.;
            let shared_mib = memory.shared() as f64 / 1024. / 1024.;

            // Capture tokio runtime information
            let metrics = Handle::current().metrics();
            let num_workers = metrics.num_workers();
            let num_alive_tasks = metrics.num_alive_tasks();
            let global_queue_depth = metrics.global_queue_depth();

            return Some(Snapshot {
                rss_mib,
                vms_mib,
                shared_mib,
                global_queue_depth,
                num_alive_tasks,
                num_workers,
            });
        }

        None
    }
}

impl Display for Snapshot {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let rss_mib = self.rss_mib;
        let vms_mib = self.vms_mib;
        let shared_mib = self.shared_mib;

        write!(fmt, "Memory usage:\x0f {rss_mib:.2} MiB\x0310 ")?;
        write!(fmt, "(VMS:\x0f {vms_mib:.2} MiB\x0310 ")?;
        write!(fmt, "Shared:\x0f {shared_mib:.2} MiB\x0310) ")?;

        write!(fmt, "Workers:\x0f {}\x0310 ", self.num_workers)?;
        write!(fmt, "Tasks:\x0f {}\x0310 ", self.num_alive_tasks)?;
        write!(fmt, "(\x0f{}\x0310 scheduled)", self.global_queue_depth)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use irc::proto::FormattedStringExt;
    use wildmatch::WildMatch;

    #[tokio::test]
    async fn it_should_format_message() {
        let snapshot = Snapshot::capture().expect("could not capture");
        let snapshot_message = snapshot.to_string().strip_formatting();
        let wildmatcher = WildMatch::new(
            "Memory usage: * MiB (VMS: * MiB Shared: * MiB) Workers: * Tasks: * (* scheduled)",
        );
        assert!(wildmatcher.matches(&snapshot_message));
    }
}
