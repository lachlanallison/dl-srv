use regex::Regex;
use std::sync::OnceLock;

static ID_PREFIX: OnceLock<Regex> = OnceLock::new();

/// Decode percent-encoding and `+` as space.
fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(byte as char);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { ' ' } else { bytes[i] as char });
        i += 1;
    }
    out
}

fn basename(raw: &str) -> &str {
    raw.trim()
        .rsplit('/')
        .next()
        .or_else(|| raw.trim().rsplit('\\').next())
        .unwrap_or(raw)
        .trim()
}

/// Strip CDN/hash prefixes and sanitize for the filesystem.
pub fn clean_download_filename(raw: &str) -> Option<String> {
    let decoded = percent_decode(raw);
    let name = basename(&decoded);
    if name.is_empty() || !name.contains('.') {
        return None;
    }

    let mut name = name.to_string();

    if let Some(caps) = ID_PREFIX
        .get_or_init(|| Regex::new(r"^[A-Za-z0-9_-]{8,}-(.+\.[A-Za-z0-9]{2,5})$").unwrap())
        .captures(&name)
    {
        name = caps[1].to_string();
    }

    name = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();

    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

pub fn filename_from_url(url: &str) -> Option<String> {
    let path = url.trim().split(['?', '#']).next()?;
    clean_download_filename(path.rsplit('/').next()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_cdn_id_prefix() {
        let raw = "coOPzNwdsNNNdI4PSde-Chef.and.My.Fridge.S02E77.1080p.NF.WEB-DL.x264.AAC2.0-LoveBug%20%5BDRAMADAY.me%5D.mkv";
        let name = clean_download_filename(raw).unwrap();
        assert_eq!(
            name,
            "Chef.and.My.Fridge.S02E77.1080p.NF.WEB-DL.x264.AAC2.0-LoveBug [DRAMADAY.me].mkv"
        );
    }

    #[test]
    fn decodes_url_encoding() {
        let name = clean_download_filename("My%20File.mkv").unwrap();
        assert_eq!(name, "My File.mkv");
    }

    #[test]
    fn filename_from_url_strips_query() {
        let url = "https://cdn.example.com/abc123xyz-Show.S01E01.mkv?token=foo";
        assert_eq!(
            filename_from_url(url).unwrap(),
            "Show.S01E01.mkv"
        );
    }
}
