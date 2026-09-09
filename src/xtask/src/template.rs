use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;

/// Replace `@VAR@` substitutions in the file (mirrors CMake `configure_file(@ONLY)`).
pub fn configure_file(src: &Path, dst: &Path, vars: &HashMap<&str, String>) -> Result<()> {
    let content =
        std::fs::read_to_string(src).with_context(|| format!("read {}", src.display()))?;
    let mut out = String::with_capacity(content.len());
    let mut rest = content.as_str();
    while let Some(i) = rest.find('@') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        if let Some(j) = after.find('@') {
            let key = &after[..j];
            if vars.contains_key(key) {
                out.push_str(&vars[key]);
                rest = &after[j + 1..];
                continue;
            }
        }
        out.push('@');
        rest = after;
    }
    out.push_str(rest);
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(dst, out).with_context(|| format!("write {}", dst.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use tempfile::tempdir;

    fn vars(pairs: &[(&'static str, &str)]) -> HashMap<&'static str, String> {
        pairs.iter().map(|(k, v)| (*k, (*v).to_owned())).collect()
    }

    fn configure(body: &str, pairs: &[(&'static str, &str)]) -> (tempfile::TempDir, String) {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("in.tmpl");
        std::fs::write(&src, body).unwrap();
        let dst = tmp.path().join("out.txt");
        configure_file(&src, &dst, &vars(pairs)).unwrap();
        let out = std::fs::read_to_string(&dst).unwrap();
        (tmp, out)
    }

    #[test]
    fn configure_file_substitutes_known_variables() {
        let (_tmp, out) = configure(
            "name=@NAME@ version=@VERSION@ again=@NAME@\n",
            &[("NAME", "astrofin"), ("VERSION", "0.4.0")],
        );
        assert_eq!(out, "name=astrofin version=0.4.0 again=astrofin\n");
    }

    #[test]
    fn configure_file_leaves_unknown_keys_verbatim() {
        let (_tmp, out) = configure("@KNOWN@ @MISSING@ end\n", &[("KNOWN", "yes")]);
        assert_eq!(out, "yes @MISSING@ end\n");
    }

    #[test]
    fn configure_file_leaves_a_lone_at_sign_verbatim() {
        let (_tmp, out) = configure("mail@example.com @NAME@ 50@\n", &[("NAME", "x")]);
        assert_eq!(out, "mail@example.com x 50@\n");
    }

    #[test]
    fn configure_file_copies_a_template_without_placeholders_unchanged() {
        let (_tmp, out) = configure("plain text, no markers\n", &[("NAME", "x")]);
        assert_eq!(out, "plain text, no markers\n");
    }

    #[test]
    fn configure_file_creates_the_destination_parent_directory() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("in.tmpl");
        std::fs::write(&src, "@NAME@\n").unwrap();
        let dst = tmp.path().join("deep").join("nested").join("out.txt");

        configure_file(&src, &dst, &vars(&[("NAME", "astrofin")])).unwrap();

        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "astrofin\n");
    }

    #[test]
    fn configure_file_errors_when_the_source_is_missing() {
        let tmp = tempdir().unwrap();
        let err = configure_file(
            &tmp.path().join("absent.tmpl"),
            &tmp.path().join("out.txt"),
            &HashMap::new(),
        )
        .expect_err("a missing template must fail");
        assert!(err.to_string().contains("read"), "{err}");
        assert!(!tmp.path().join("out.txt").exists());
    }
}
