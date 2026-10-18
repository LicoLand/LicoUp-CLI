use anyhow::{Result, anyhow, bail, ensure};
use zeroize::Zeroizing;

use super::{
    constants::{
        MAX_CONTENT_BYTES, MAX_CONTENT_TYPE_BYTES, MAX_CONTEXT_FIELD_BYTES, PLAINTEXT_MAGIC,
        PRIVATE_CONTEXT_FRAME_MAGIC,
    },
    length_codec::{SliceReader, append_len_prefixed_bytes},
    model::{
        OpenedSecureMeshPayload, OpenedSecureMeshPrivateContextPayload, SecureMeshContentContext,
        SecureMeshPayloadKind, SecureMeshPlaintext,
    },
    validation::validate_plaintext,
};

pub(super) fn encode_private_context_frame(
    context: &SecureMeshContentContext,
    plaintext: &SecureMeshPlaintext,
) -> Result<Zeroizing<Vec<u8>>> {
    context.validate()?;
    validate_plaintext(plaintext)?;
    let mut out = Zeroizing::new(Vec::new());
    out.extend_from_slice(PRIVATE_CONTEXT_FRAME_MAGIC);
    out.push(plaintext.kind.tag());
    append_len_prefixed_bytes(&mut out, context.envelope_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, context.message_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, context.opaque_mailbox_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, context.sender_endpoint_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, context.recipient_endpoint_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, context.session_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, context.created_at.as_bytes())?;
    append_len_prefixed_bytes(&mut out, context.expires_at.as_bytes())?;
    match &plaintext.content_type {
        Some(content_type) => {
            out.push(1);
            append_len_prefixed_bytes(&mut out, content_type.as_bytes())?;
        }
        None => out.push(0),
    }
    append_len_prefixed_bytes(&mut out, &plaintext.body)?;
    Ok(out)
}

pub(super) fn decode_private_context_frame(
    bytes: &[u8],
) -> Result<OpenedSecureMeshPrivateContextPayload> {
    let mut reader = SliceReader::new(bytes);
    reader.expect_bytes(PRIVATE_CONTEXT_FRAME_MAGIC)?;
    let kind = SecureMeshPayloadKind::from_tag(reader.read_u8()?)?;
    let envelope_id = read_bounded_required_string(
        &mut reader,
        "private-context envelope_id",
        MAX_CONTEXT_FIELD_BYTES,
    )?;
    let message_id = read_bounded_required_string(
        &mut reader,
        "private-context message_id",
        MAX_CONTEXT_FIELD_BYTES,
    )?;
    let opaque_mailbox_id = read_bounded_required_string(
        &mut reader,
        "private-context opaque_mailbox_id",
        MAX_CONTEXT_FIELD_BYTES,
    )?;
    let sender_endpoint_id = read_bounded_required_string(
        &mut reader,
        "private-context sender_endpoint_id",
        MAX_CONTEXT_FIELD_BYTES,
    )?;
    let recipient_endpoint_id = read_bounded_required_string(
        &mut reader,
        "private-context recipient_endpoint_id",
        MAX_CONTEXT_FIELD_BYTES,
    )?;
    let session_id = read_bounded_required_string(
        &mut reader,
        "private-context session_id",
        MAX_CONTEXT_FIELD_BYTES,
    )?;
    let created_at = read_bounded_required_string(
        &mut reader,
        "private-context created_at",
        MAX_CONTEXT_FIELD_BYTES,
    )?;
    let expires_at = read_bounded_required_string(
        &mut reader,
        "private-context expires_at",
        MAX_CONTEXT_FIELD_BYTES,
    )?;
    let content_type = match reader.read_u8()? {
        0 => None,
        1 => Some(read_bounded_required_string(
            &mut reader,
            "private-context content_type",
            MAX_CONTENT_TYPE_BYTES,
        )?),
        _ => bail!("secure mesh private-context content type marker is unsupported"),
    };
    let body = reader.read_len_prefixed_bytes()?;
    ensure!(
        body.len() <= MAX_CONTENT_BYTES,
        "secure mesh private-context payload body is too large"
    );
    let body = body.to_vec();
    ensure!(
        reader.is_empty(),
        "secure mesh private-context frame has trailing bytes"
    );
    let context = SecureMeshContentContext {
        envelope_id,
        message_id,
        opaque_mailbox_id,
        sender_endpoint_id,
        recipient_endpoint_id,
        session_id,
        created_at,
        expires_at,
    };
    context.validate()?;
    let payload = OpenedSecureMeshPayload {
        kind,
        body,
        content_type,
        created_at: context.created_at.clone(),
        expires_at: context.expires_at.clone(),
    };
    Ok(OpenedSecureMeshPrivateContextPayload { context, payload })
}

fn read_bounded_required_string(
    reader: &mut SliceReader<'_>,
    label: &str,
    maximum_bytes: usize,
) -> Result<String> {
    let bytes = reader.read_len_prefixed_bytes()?;
    ensure!(
        !bytes.is_empty() && bytes.len() <= maximum_bytes,
        "secure mesh {label} is outside bounds"
    );
    let value = String::from_utf8(bytes.to_vec())
        .map_err(|_| anyhow!("secure mesh {label} is not valid UTF-8"))?;
    ensure!(!value.trim().is_empty(), "secure mesh {label} is required");
    Ok(value)
}

pub(super) fn encode_plaintext(
    context: &SecureMeshContentContext,
    plaintext: &SecureMeshPlaintext,
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(PLAINTEXT_MAGIC);
    out.push(plaintext.kind.tag());
    append_len_prefixed_bytes(&mut out, context.created_at.as_bytes())?;
    append_len_prefixed_bytes(&mut out, context.expires_at.as_bytes())?;
    match &plaintext.content_type {
        Some(content_type) => {
            out.push(1);
            append_len_prefixed_bytes(&mut out, content_type.as_bytes())?;
        }
        None => out.push(0),
    }
    append_len_prefixed_bytes(&mut out, &plaintext.body)?;
    Ok(out)
}

pub(super) fn decode_plaintext(bytes: &[u8]) -> Result<OpenedSecureMeshPayload> {
    let mut reader = SliceReader::new(bytes);
    reader.expect_bytes(PLAINTEXT_MAGIC)?;
    let kind = SecureMeshPayloadKind::from_tag(reader.read_u8()?)?;
    let created_at = read_string(&mut reader, "created_at")?;
    let expires_at = read_string(&mut reader, "expires_at")?;
    let content_type = match reader.read_u8()? {
        0 => None,
        1 => Some(read_string(&mut reader, "content_type")?),
        _ => bail!("secure mesh payload content type marker is unsupported"),
    };
    let body = reader.read_len_prefixed_bytes()?.to_vec();
    ensure!(
        reader.is_empty(),
        "secure mesh payload has trailing plaintext bytes"
    );
    ensure!(
        body.len() <= MAX_CONTENT_BYTES,
        "secure mesh payload body is too large"
    );
    Ok(OpenedSecureMeshPayload {
        kind,
        body,
        content_type,
        created_at,
        expires_at,
    })
}

fn read_string(reader: &mut SliceReader<'_>, label: &str) -> Result<String> {
    let bytes = reader.read_len_prefixed_bytes()?;
    String::from_utf8(bytes.to_vec())
        .map_err(|_| anyhow!("secure mesh payload {label} is not valid UTF-8"))
}
