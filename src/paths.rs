use std::path::{Component, Path, PathBuf};

use crate::PathStyle;

pub(crate) fn display(path: &Path, cwd: &Path, style: PathStyle) -> String {
    if matches!(style, PathStyle::Native) {
        return path.to_string_lossy().into_owned();
    }
    let absolute = normalize(&if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    });
    let selected = if matches!(style, PathStyle::Relative) {
        relative(&absolute, &normalize(cwd)).unwrap_or(absolute)
    } else {
        absolute
    };
    let text = selected.to_string_lossy().into_owned();
    if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text
    }
}

fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            component => result.push(component.as_os_str()),
        }
    }
    result
}

fn relative(path: &Path, base: &Path) -> Option<PathBuf> {
    let target: Vec<_> = path.components().collect();
    let source: Vec<_> = base.components().collect();
    let same = |a: &Component<'_>, b: &Component<'_>| {
        if cfg!(windows) {
            a.as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
        } else {
            a == b
        }
    };
    if !same(target.first()?, source.first()?) {
        return None;
    }
    let common = target
        .iter()
        .zip(&source)
        .take_while(|(a, b)| same(a, b))
        .count();
    let mut result = PathBuf::new();
    for _ in common..source.len() {
        result.push("..");
    }
    for part in &target[common..] {
        result.push(part.as_os_str());
    }
    if result.as_os_str().is_empty() {
        result.push(".");
    }
    Some(result)
}
