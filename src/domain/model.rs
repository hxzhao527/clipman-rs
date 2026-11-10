use std::collections::BTreeMap;
use std::str::FromStr;

use encoding_rs::{Encoding, UTF_8};
use mime::Mime;
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const MAX_CAPTURE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Representation {
    pub mime_type: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapturedPayload {
    pub fingerprint: [u8; 32],
    pub plain_text: String,
    pub representations: Vec<Representation>,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CaptureReject {
    #[error("clipboard has no text/plain representation")]
    NoTextPlain,
    #[error("text/plain is empty after trimming whitespace")]
    EmptyText,
    #[error("captured text representations exceed the 4 MiB limit")]
    TooLarge,
}

pub fn is_text_mime(mime_type: &str) -> bool {
    parsed_mime(mime_type).is_some_and(|mime| mime.type_() == mime::TEXT)
}

pub fn is_text_plain_mime(mime_type: &str) -> bool {
    parsed_mime(mime_type)
        .is_some_and(|mime| mime.type_() == mime::TEXT && mime.subtype() == mime::PLAIN)
}

pub fn build_payload(
    representations: Vec<Representation>,
) -> Result<CapturedPayload, CaptureReject> {
    let mut text_representations = BTreeMap::<String, Vec<u8>>::new();
    for representation in representations {
        if is_text_mime(&representation.mime_type) {
            text_representations
                .entry(representation.mime_type)
                .or_insert(representation.bytes);
        }
    }

    let plain = text_representations
        .iter()
        .filter(|(mime_type, _)| is_text_plain_mime(mime_type))
        .min_by_key(|(mime_type, _)| plain_preference(mime_type))
        .map(|(mime_type, bytes)| (mime_type.clone(), bytes.clone()))
        .ok_or(CaptureReject::NoTextPlain)?;

    let total_size = text_representations
        .values()
        .try_fold(0usize, |total, bytes| total.checked_add(bytes.len()))
        .ok_or(CaptureReject::TooLarge)?;
    if total_size > MAX_CAPTURE_BYTES {
        return Err(CaptureReject::TooLarge);
    }

    let plain_text = decode_text(&plain.0, &plain.1);
    if plain_text.trim_matches(char::is_whitespace).is_empty() {
        return Err(CaptureReject::EmptyText);
    }

    let representations = text_representations
        .into_iter()
        .map(|(mime_type, bytes)| Representation { mime_type, bytes })
        .collect::<Vec<_>>();
    let fingerprint = fingerprint(&representations);

    Ok(CapturedPayload {
        fingerprint,
        plain_text,
        representations,
    })
}

fn parsed_mime(mime_type: &str) -> Option<Mime> {
    Mime::from_str(mime_type).ok()
}

fn plain_preference(mime_type: &str) -> (u8, String) {
    let charset = parsed_mime(mime_type)
        .and_then(|mime| {
            mime.get_param(mime::CHARSET)
                .map(|charset| charset.as_str().to_owned())
        })
        .unwrap_or_default();
    let rank = if charset.eq_ignore_ascii_case("utf-8") || charset.eq_ignore_ascii_case("utf8") {
        0
    } else if charset.is_empty() {
        1
    } else {
        2
    };
    (rank, charset.to_ascii_lowercase())
}

fn decode_text(mime_type: &str, bytes: &[u8]) -> String {
    let encoding = parsed_mime(mime_type)
        .and_then(|mime| {
            mime.get_param(mime::CHARSET)
                .and_then(|charset| Encoding::for_label(charset.as_str().as_bytes()))
        })
        .unwrap_or(UTF_8);
    let (decoded, _, _) = encoding.decode(bytes);
    decoded.into_owned()
}

fn fingerprint(representations: &[Representation]) -> [u8; 32] {
    let mut representations = representations.to_vec();
    representations.sort_by(|left, right| left.mime_type.cmp(&right.mime_type));

    let mut hasher = Sha256::new();
    hasher.update(b"clipman-rs/text-payload/v1\0");
    for representation in representations {
        hasher.update((representation.mime_type.len() as u64).to_be_bytes());
        hasher.update(representation.mime_type.as_bytes());
        hasher.update((representation.bytes.len() as u64).to_be_bytes());
        hasher.update(representation.bytes);
    }
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn representation(mime_type: &str, bytes: &[u8]) -> Representation {
        Representation {
            mime_type: mime_type.to_owned(),
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn text_mime_detection_uses_media_type() {
        assert!(is_text_mime("text/plain;charset=UTF-8"));
        assert!(is_text_mime("text/html"));
        assert!(!is_text_mime("application/json"));
        assert!(is_text_plain_mime("text/plain;charset=utf-8"));
        assert!(!is_text_plain_mime("text/html"));
    }

    #[test]
    fn captures_all_text_representations() {
        let payload = build_payload(vec![
            representation("text/plain;charset=utf-8", b"hello"),
            representation("text/html", b"<b>hello</b>"),
            representation("application/json", b"{\"hello\":true}"),
        ])
        .unwrap();

        assert_eq!(payload.plain_text, "hello");
        assert_eq!(payload.representations.len(), 2);
        assert!(
            payload
                .representations
                .iter()
                .all(|entry| is_text_mime(&entry.mime_type))
        );
    }

    #[test]
    fn rejects_payload_without_text_plain() {
        let result = build_payload(vec![representation("text/html", b"<b>hello</b>")]);
        assert_eq!(result, Err(CaptureReject::NoTextPlain));
    }

    #[test]
    fn rejects_text_that_is_only_whitespace() {
        let result = build_payload(vec![representation(
            "text/plain;charset=utf-8",
            "\t\r\n  ".as_bytes(),
        )]);
        assert_eq!(result, Err(CaptureReject::EmptyText));
    }

    #[test]
    fn preserves_original_whitespace_in_stored_text() {
        let payload = build_payload(vec![representation("text/plain", b"  hello\n")]).unwrap();
        assert_eq!(payload.plain_text, "  hello\n");
    }

    #[test]
    fn rejects_representation_set_over_limit() {
        let mut bytes = vec![b'a'; MAX_CAPTURE_BYTES];
        bytes.push(b'b');
        let result = build_payload(vec![representation("text/plain", &bytes)]);
        assert_eq!(result, Err(CaptureReject::TooLarge));
    }

    #[test]
    fn fingerprint_is_order_independent() {
        let first = build_payload(vec![
            representation("text/plain", b"hello"),
            representation("text/html", b"<b>hello</b>"),
        ])
        .unwrap();
        let second = build_payload(vec![
            representation("text/html", b"<b>hello</b>"),
            representation("text/plain", b"hello"),
        ])
        .unwrap();

        assert_eq!(first.fingerprint, second.fingerprint);
    }

    #[test]
    fn fingerprint_changes_with_rich_text() {
        let first = build_payload(vec![
            representation("text/plain", b"hello"),
            representation("text/html", b"<b>hello</b>"),
        ])
        .unwrap();
        let second = build_payload(vec![
            representation("text/plain", b"hello"),
            representation("text/html", b"<i>hello</i>"),
        ])
        .unwrap();

        assert_ne!(first.fingerprint, second.fingerprint);
    }
}
