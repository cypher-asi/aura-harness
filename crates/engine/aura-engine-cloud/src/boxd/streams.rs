//! boxd's streaming calls: `Exec` (streamed both ways), and file uploads
//! and downloads.

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{stream, StreamExt as _};

use super::{BoxdProvider, CREATE_TIMEOUT};
use crate::error::{CloudError, CloudResult};
use crate::proto::{self, boxd_api_client::BoxdApiClient};
use crate::provider::ExecOutput;

/// Largest chunk sent in one upload message (boxd caps a message at
/// 4 MiB).
const UPLOAD_CHUNK: usize = 1024 * 1024;
/// How often an upload says how far it has got.
const UPLOAD_REPORT_EVERY: Duration = Duration::from_millis(500);

const EXEC_OP: &str = "Exec";
const UPLOAD_OP: &str = "UploadFileStream";
const DOWNLOAD_OP: &str = "DownloadFileStream";

impl BoxdProvider {
    /// `Exec`: run `command` on `machine` with `stdin`, handing each
    /// output chunk to `on_chunk` as it arrives.
    pub(super) async fn exec_stream(
        &self,
        machine: &str,
        command: &str,
        stdin: Option<Vec<u8>>,
        timeout: Duration,
        on_chunk: &mut (dyn for<'chunk> FnMut(bool, &'chunk [u8]) + Send),
    ) -> CloudResult<ExecOutput> {
        let channel = self.channel(&self.endpoint_of(machine))?;
        let mut chunks = vec![proto::ExecChunk {
            vm_id: machine.to_string(),
            command: command.to_string(),
            stdin: stdin.is_some(),
            ..Default::default()
        }];
        if let Some(input) = stdin.filter(|input| !input.is_empty()) {
            chunks.push(proto::ExecChunk {
                data: input,
                stdin: true,
                ..Default::default()
            });
        }
        let request = self.request(stream::iter(chunks), timeout).await?;
        let run = async {
            let mut answer = BoxdApiClient::new(channel)
                .exec(request)
                .await
                .map_err(|status| CloudError::rpc(EXEC_OP, &status))?
                .into_inner();
            let mut output = ExecOutput::default();
            while let Some(chunk) = answer
                .message()
                .await
                .map_err(|status| CloudError::rpc(EXEC_OP, &status))?
            {
                if !chunk.data.is_empty() {
                    on_chunk(chunk.is_stderr, &chunk.data);
                }
                if chunk.is_stderr {
                    output.stderr.extend_from_slice(&chunk.data);
                } else {
                    output.stdout.extend_from_slice(&chunk.data);
                }
                output.exit_code = chunk.exit_code;
            }
            Ok::<_, CloudError>(output)
        };
        tokio::time::timeout(timeout, run)
            .await
            .map_err(|_| CloudError::Timeout {
                op: EXEC_OP,
                after: timeout,
            })?
    }

    /// `UploadFileStream`: `bytes` to `path` on `machine`, telling
    /// `on_sent` how far it has got every [`UPLOAD_REPORT_EVERY`].
    pub(super) async fn upload_stream(
        &self,
        machine: &str,
        path: &str,
        bytes: Vec<u8>,
        on_sent: &mut (dyn FnMut(u64, u64) + Send),
    ) -> CloudResult<u64> {
        let total = bytes.len() as u64;
        // Counted as tonic takes each chunk off the stream to send it:
        // with HTTP/2 flow control that tracks what went out closely.
        let sent = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&sent);
        let chunks = stream::iter(upload_chunks(machine, path, &bytes)).inspect(move |chunk| {
            counter.fetch_add(chunk.data.len() as u64, Ordering::Relaxed);
        });
        let channel = self.channel(&self.endpoint_of(machine))?;
        let request = self.request(chunks, CREATE_TIMEOUT).await?;
        let mut client = BoxdApiClient::new(channel);
        let call = tokio::time::timeout(CREATE_TIMEOUT, client.upload_file_stream(request));
        let written = report_while(call, &sent, total, on_sent)
            .await
            .map_err(|_| CloudError::Timeout {
                op: UPLOAD_OP,
                after: CREATE_TIMEOUT,
            })?
            .map_err(|status| CloudError::rpc(UPLOAD_OP, &status))?
            .into_inner()
            .bytes_written;
        if written != total {
            return Err(CloudError::InvalidResponse {
                op: UPLOAD_OP,
                message: format!("wrote {written} of {total} bytes to {path}"),
            });
        }
        on_sent(total, total);
        Ok(written)
    }

    /// `DownloadFileStream`: `path` on `machine`, refused above
    /// `max_bytes`.
    pub(super) async fn download_stream(
        &self,
        machine: &str,
        path: &str,
        max_bytes: u64,
    ) -> CloudResult<Vec<u8>> {
        let channel = self.channel(&self.endpoint_of(machine))?;
        let request = self
            .request(
                proto::DownloadFileRequest {
                    vm_id: machine.to_string(),
                    path: path.to_string(),
                },
                CREATE_TIMEOUT,
            )
            .await?;
        let run = async {
            let answer = BoxdApiClient::new(channel)
                .download_file_stream(request)
                .await
                .map_err(|status| CloudError::rpc(DOWNLOAD_OP, &status))?
                .into_inner();
            read_download(answer, path, max_bytes).await
        };
        tokio::time::timeout(CREATE_TIMEOUT, run)
            .await
            .map_err(|_| CloudError::Timeout {
                op: DOWNLOAD_OP,
                after: CREATE_TIMEOUT,
            })?
    }
}

/// An upload's messages: the first names the machine, the path and the
/// size, the rest carry the bytes.
fn upload_chunks(machine: &str, path: &str, bytes: &[u8]) -> Vec<proto::UploadFileChunk> {
    let mut chunks = vec![proto::UploadFileChunk {
        vm_id: machine.to_string(),
        path: path.to_string(),
        total_size: bytes.len() as u64,
        data: Vec::new(),
    }];
    chunks.extend(
        bytes
            .chunks(UPLOAD_CHUNK)
            .map(|data| proto::UploadFileChunk {
                data: data.to_vec(),
                ..Default::default()
            }),
    );
    chunks
}

/// Await `call`, telling `on_sent` how much of `total` has gone out
/// every [`UPLOAD_REPORT_EVERY`] until it ends.
async fn report_while<F: Future>(
    call: F,
    sent: &AtomicU64,
    total: u64,
    on_sent: &mut (dyn FnMut(u64, u64) + Send),
) -> F::Output {
    tokio::pin!(call);
    let mut every = tokio::time::interval(UPLOAD_REPORT_EVERY);
    loop {
        tokio::select! {
            answer = &mut call => return answer,
            _ = every.tick() => {
                on_sent(sent.load(Ordering::Relaxed).min(total), total);
            }
        }
    }
}

/// A download's bytes: refused when the file is above `max_bytes`, and
/// when fewer bytes arrive than it has.
async fn read_download(
    mut answer: tonic::Streaming<proto::DownloadFileChunk>,
    path: &str,
    max_bytes: u64,
) -> CloudResult<Vec<u8>> {
    let mut total = None;
    let mut bytes = Vec::new();
    while let Some(chunk) = answer
        .message()
        .await
        .map_err(|status| CloudError::rpc(DOWNLOAD_OP, &status))?
    {
        if total.is_none() {
            if chunk.total_size > max_bytes {
                return Err(CloudError::InvalidResponse {
                    op: DOWNLOAD_OP,
                    message: format!(
                        "{path} is {} bytes, above the {max_bytes} byte limit",
                        chunk.total_size
                    ),
                });
            }
            total = Some(chunk.total_size);
        }
        bytes.extend_from_slice(&chunk.data);
    }
    let expected = total.unwrap_or(0);
    if bytes.len() as u64 != expected {
        return Err(CloudError::InvalidResponse {
            op: DOWNLOAD_OP,
            message: format!("got {} of {expected} bytes of {path}", bytes.len()),
        });
    }
    Ok(bytes)
}
