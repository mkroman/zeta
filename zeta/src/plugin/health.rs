use std::fmt::Display;

use async_trait::async_trait;
use irc::client::Client;
use irc::proto::{Command, Message};
use psutil::process::Process;
use tokio::runtime::Handle;

use crate::Error as ZetaError;

use super::{Author, Name, Plugin, Version};

pub struct Health;

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
        if let Command::PRIVMSG(ref channel, ref message) = message.command {
            if message.starts_with(".health") {
                client.send_privmsg(channel, self.to_string())?;
            }
        }

        Ok(())
    }
}

impl Display for Health {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "\x0310>\x02\x03 Health:\x02\x0310 ")?;

        if let Ok(proc) = Process::current() {
            if let Ok(memory) = proc.memory_info() {
                let rss_mib = memory.rss() as f64 / 1024. / 1024.;
                let vms_mib = memory.vms() as f64 / 1024. / 1024.;
                let shared_mib = memory.shared() as f64 / 1024. / 1024.;

                write!(
                    f,
                    "Memory usage:\x0f {:.2} MiB\x0310 (VMS:\x0f {:.2} MiB\x0310 Shared:\x0f {:.2} MiB\x0310) ",
                    rss_mib, vms_mib, shared_mib
                )?;
            }
        }

        let metrics = Handle::current().metrics();
        let num_workers = metrics.num_workers();
        let num_alive_tasks = metrics.num_alive_tasks();
        let global_queue_depth = metrics.global_queue_depth();

        write!(f, "Workers:\x0f {num_workers}\x0310 ")?;
        write!(f, "Tasks:\x0f {num_alive_tasks}\x0310 ")?;
        write!(f, "(\x0f{global_queue_depth}\x0310 scheduled) ")?;

        Ok(())
    }
}
