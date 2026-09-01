//! The file service: invoice attachments and stock exports.
//!
//! Three ways to upload, from most to least help:
//!
//! | Method | Does for you | Use when |
//! |---|---|---|
//! | `upload_file` | chunks, hashes, checks the size | the file is in memory |
//! | `upload_file_chunked` | re-chunks a stream, checks the declared total | the source is a stream and you know size and hash up front |
//! | `upload_file_stream` | nothing | you build every message yourself |
//!
//! The wire contract the first two honour for you: the **first** message
//! carries the metadata (tenant, bucket, file id, `size_bytes`,
//! `content_type`, `checksum`, `request_id`); later messages are read only
//! for their `chunk`; no chunk may exceed 1 MiB; the chunks must add up to
//! `size_bytes` exactly; and `checksum` must be exactly 32 bytes — the
//! server checks its length, never its content.

use crate::{BUCKET, Erp, Result};
use rociadb_sdk::{FileStreamUploadOptions, FileUploadOptions, StatResponse, UploadRequest};
use sha2::{Digest, Sha256};

/// Upload a file already in memory.
///
/// With `checksum: None`, `upload_file` hashes the buffer itself and slices
/// it into 1 MiB messages. We pass it explicitly here to show the rule: it
/// must be exactly 32 bytes, or the call fails client-side before sending.
pub async fn upload(erp: &Erp, file_id: &str, content: &[u8], content_type: &str) -> Result<()> {
    erp.client
        .upload_file(
            &erp.tenant,
            BUCKET,
            file_id,
            content,
            FileUploadOptions {
                content_type: content_type.to_string(),
                checksum: Some(Sha256::digest(content).to_vec()),
                request_id: Some(erp.key(&format!("upload:{file_id}"))),
            },
        )
        .await?;
    Ok(())
}

/// Upload content produced in pieces, without holding all of it at once.
///
/// `size_bytes` and `checksum` travel on the very first gRPC message, before
/// a single byte has been read from the stream, so both must be known up
/// front. If the stream ends up producing a different total, this fails
/// client-side rather than at the end of the upload.
///
/// The pieces here are 64 KiB; `upload_file_chunked` re-buffers them into
/// 1 MiB messages whatever size they arrive in.
pub async fn upload_streamed(
    erp: &Erp,
    file_id: &str,
    content: Vec<u8>,
    content_type: &str,
) -> Result<()> {
    let size = content.len() as u64;
    let checksum = Sha256::digest(&content).to_vec();
    let pieces: Vec<Vec<u8>> = content.chunks(64 * 1024).map(<[u8]>::to_vec).collect();

    erp.client
        .upload_file_chunked(
            &erp.tenant,
            BUCKET,
            file_id,
            size,
            checksum,
            futures::stream::iter(pieces),
            FileStreamUploadOptions {
                content_type: content_type.to_string(),
                request_id: Some(erp.key(&format!("export:{file_id}"))),
            },
        )
        .await?;
    Ok(())
}

/// Upload by building the protobuf message yourself.
///
/// `upload_file_stream` is the low-level escape hatch: no re-chunking, no
/// size cap applied, no checksum computed. A wrong `size_bytes`, or a
/// checksum that does not match the bytes, goes through silently — the
/// server only checks the checksum's length. This note fits in one message,
/// which is the only case worth hand-writing.
pub async fn upload_raw(erp: &Erp, file_id: &str, content: &[u8]) -> Result<()> {
    if content.len() > 1024 * 1024 {
        return Err("upload_raw only handles content that fits in one 1 MiB message".into());
    }
    let request = UploadRequest {
        tenant_id: erp.tenant.clone(),
        bucket: BUCKET.to_string(),
        file_id: file_id.to_string(),
        size_bytes: content.len() as u64,
        content_type: "text/plain".to_string(),
        checksum: Sha256::digest(content).to_vec(),
        chunk: content.to_vec(),
        request_id: erp.key(&format!("raw:{file_id}")),
    };

    erp.client
        .upload_file_stream(futures::stream::iter([request]))
        .await?;
    Ok(())
}

/// Metadata without downloading the file.
pub async fn stat(erp: &Erp, file_id: &str) -> Result<StatResponse> {
    Ok(erp.client.stat_file(&erp.tenant, BUCKET, file_id).await?)
}

/// Download the whole file.
pub async fn download(erp: &Erp, file_id: &str) -> Result<Vec<u8>> {
    Ok(erp
        .client
        .download_file(&erp.tenant, BUCKET, file_id)
        .await?)
}

/// Download as a stream, never holding the whole file in memory.
///
/// This is the one place where an error arrives as a raw `tonic::Status`
/// instead of a `RociaDbError`. Counting bytes here, but this is the same
/// loop you would write to pipe it to a file.
pub async fn download_streamed(erp: &Erp, file_id: &str) -> Result<u64> {
    let mut stream = erp
        .client
        .download_file_stream(&erp.tenant, BUCKET, file_id)
        .await?;

    let mut bytes = 0;
    while let Some(response) = stream.message().await? {
        bytes += response.chunk.len() as u64;
    }
    Ok(bytes)
}

/// The buckets in this tenant.
pub async fn list_buckets(erp: &Erp) -> Result<Vec<String>> {
    let page = erp.client.list_buckets(&erp.tenant, Some(50), None).await?;
    Ok(page.items)
}

/// The files in our bucket.
pub async fn list_files(erp: &Erp) -> Result<Vec<String>> {
    let page = erp
        .client
        .list_files(&erp.tenant, BUCKET, Some(50), None)
        .await?;
    Ok(page.items)
}

/// Delete a file. Idempotent, like `delete_document`.
pub async fn delete(erp: &Erp, file_id: &str) -> Result<()> {
    erp.client.delete_file(&erp.tenant, BUCKET, file_id).await?;
    Ok(())
}

/// Delete a file with a chosen key.
pub async fn delete_with_key(erp: &Erp, file_id: &str) -> Result<()> {
    erp.client
        .delete_file_with_request_id(
            &erp.tenant,
            BUCKET,
            file_id,
            erp.key(&format!("delete-file:{file_id}")),
        )
        .await?;
    Ok(())
}
