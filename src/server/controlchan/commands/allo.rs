//! The RFC959 Allocate (`ALLO`) command
//
// This command may be required by some servers to reserve
// sufficient storage to accommodate the new file to be
// transferred.  The argument shall be a decimal integer
// representing the number of bytes (using the logical byte
// size) of storage to be reserved for the file.  For files
// sent with record or page structure a maximum record or page
// size (in logical bytes) might also be necessary; this is
// indicated by a decimal integer in a second argument field of
// the command.  This second argument is optional, but when
// present should be separated from the first by the three
// Telnet characters <SP> R <SP>.  This command shall be
// followed by a STORe or APPEnd command.  The ALLO command
// should be treated as a NOOP (no operation) by those servers
// which do not require that the maximum size of the file be
// declared beforehand, and those servers interested in only
// the maximum record or page size should accept a dummy value
// in the first argument and ignore it.

use crate::auth::UserDetail;
use crate::server::controlchan::error::ControlChanError;
use crate::server::controlchan::handler::CommandContext;
use crate::server::controlchan::handler::CommandHandler;
use crate::server::controlchan::{Reply, ReplyCode};
use crate::storage;
use async_trait::async_trait;

pub struct Allo;

#[async_trait]
impl<S, U> CommandHandler<S, U> for Allo
where
    U: UserDetail + 'static,
    S: 'static + storage::StorageBackend<U> + Sync + Send,
    S::File: tokio::io::AsyncRead + Send,
    S::Metadata: storage::Metadata,
{
    async fn handle(&self, _args: CommandContext<S, U>) -> Result<Reply, ControlChanError> {
        // ALLO is obsolete and we'll just ignore it.
        Ok(Reply::new(ReplyCode::CommandOkayNotImplemented, "Ignored"))
    }
}
