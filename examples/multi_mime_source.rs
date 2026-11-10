use std::thread;
use std::time::Duration;

use anyhow::Result;
use wl_clipboard_rs::copy::{MimeSource, MimeType, Options, Source};

fn main() -> Result<()> {
    let mut options = Options::new();
    options.omit_additional_text_mime_types(true);
    options.copy_multi(vec![
        MimeSource {
            source: Source::Bytes(b"clipman-rs multi MIME fixture"[..].into()),
            mime_type: MimeType::Specific("text/plain".to_owned()),
        },
        MimeSource {
            source: Source::Bytes(b"clipman-rs multi MIME fixture"[..].into()),
            mime_type: MimeType::Specific("text/plain;charset=utf-8".to_owned()),
        },
        MimeSource {
            source: Source::Bytes(b"<b>clipman-rs</b> <i>multi MIME fixture</i>"[..].into()),
            mime_type: MimeType::Specific("text/html".to_owned()),
        },
        MimeSource {
            source: Source::Bytes(br"{\rtf1\ansi clipman-rs \b multi MIME\b0  fixture}"[..].into()),
            mime_type: MimeType::Specific("text/rtf".to_owned()),
        },
    ])?;

    println!("multi-MIME clipboard source is running; press Ctrl+C to stop");
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}
