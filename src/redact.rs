//! Removes the values of secret variables from the text that jlog logs.

/// The variables whose values jlog may log. The names are exact, not prefixes.
pub const SAFE: [&str; 8] = [
    "PATH",
    "SHELL",
    "HOME",
    "TERM",
    "CI",
    "CODEX_THREAD_ID",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
];

/// A value shorter than this is not treated as a secret.
pub const MIN_LEN: usize = 8;

pub const MARK: &str = "<redacted>";

/// The values of the variables that are not on the safe list and have 8 or
/// more characters.
pub fn secrets<I, K, V>(vars: I) -> Vec<String>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<std::ffi::OsStr>,
    V: AsRef<std::ffi::OsStr>,
{
    let mut out: Vec<String> = vars
        .into_iter()
        .filter(|(k, _)| !k.as_ref().to_str().is_some_and(|k| SAFE.contains(&k)))
        // A value pasted into a secret store often ends with a newline.
        .map(|(_, v)| v.as_ref().to_string_lossy().trim().to_string())
        .filter(|v| v.chars().count() >= MIN_LEN)
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Replaces each place where a secret occurs in `text` with `<redacted>`.
/// Overlapping and adjacent places become one mark.
pub fn redact(text: &str, secrets: &[String]) -> String {
    let mut hit = vec![false; text.len()];
    let mut any = false;
    for s in secrets {
        // Each start, also starts inside an earlier match: match_indices
        // finds only matches that do not overlap.
        let mut from = 0;
        while let Some(at) = text[from..].find(s.as_str()) {
            let i = from + at;
            hit[i..i + s.len()].iter_mut().for_each(|h| *h = true);
            any = true;
            from = i + text[i..].chars().next().map_or(1, char::len_utf8);
        }
    }
    if !any {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if hit[i] {
            while i < text.len() && hit[i] {
                i += 1;
            }
            out.push_str(MARK);
        } else {
            // A secret is valid UTF-8, so each run of hits starts and ends on a
            // character boundary.
            let c = text[i..].chars().next().expect("on a char boundary");
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn keeps_safe_and_short_values() {
        let vars = [
            ("PATH", "/usr/bin:/bin"),
            ("HOME", "/Users/someone"),
            ("SHORT", "abc"),
            ("TOKEN", "s3cr3t-value"),
        ];
        assert_eq!(secrets(vars), s(&["s3cr3t-value"]));
    }

    #[test]
    fn redacts_every_place() {
        let got = redact("curl -H s3cr3t-value x s3cr3t-value", &s(&["s3cr3t-value"]));
        assert_eq!(got, "curl -H <redacted> x <redacted>");
    }

    #[test]
    fn overlapping_secrets_make_one_mark() {
        let got = redact("x abcdefghijkl y", &s(&["abcdefgh", "efghijkl"]));
        assert_eq!(got, "x <redacted> y");
    }

    #[test]
    fn a_secret_that_is_part_of_the_mark_does_not_nest() {
        let got = redact("a 12345678 b", &s(&["12345678", "redacted"]));
        assert_eq!(got, "a <redacted> b");
    }

    #[test]
    fn trims_values_before_matching() {
        let got = secrets([("TOKEN", "s3cr3t-value\n"), ("BLANK", "        ")]);
        assert_eq!(got, s(&["s3cr3t-value"]));
    }

    #[test]
    fn redacts_overlapping_places_of_one_secret() {
        let got = redact("x abcabcabcab y", &s(&["abcabcab"]));
        assert_eq!(got, "x <redacted> y");
    }

    #[test]
    fn handles_multibyte_text() {
        let got = redact("é s3cr3t-value ü", &s(&["s3cr3t-value"]));
        assert_eq!(got, "é <redacted> ü");
    }
}
