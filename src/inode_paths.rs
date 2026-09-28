//! FUSE inode-to-path bookkeeping. APFS directory size is not a fixed host cache limit.
use anyhow::{Context, Result};
use std::collections::HashMap;

struct Entry {
    path: String,
    lookups: u64,
}

pub(crate) struct InodePaths {
    entries: HashMap<u64, Entry>,
}

impl InodePaths {
    pub(crate) fn new() -> Self {
        Self {
            entries: HashMap::from([(
                1,
                Entry {
                    path: "/".into(),
                    lookups: 1,
                },
            )]),
        }
    }

    pub(crate) fn get(&self, ino: u64) -> Option<&str> {
        self.entries.get(&ino).map(|e| e.path.as_str())
    }

    pub(crate) fn remember(&mut self, ino: u64, path: String) -> Result<()> {
        if let Some(entry) = self.entries.get_mut(&ino) {
            entry.lookups = entry
                .lookups
                .checked_add(1)
                .context("FUSE lookup count overflow")?;
        } else {
            self.entries
                .try_reserve(1)
                .map_err(|_| std::io::Error::from_raw_os_error(libc::ENOMEM))?;
            self.entries.insert(ino, Entry { path, lookups: 1 });
        }
        Ok(())
    }

    pub(crate) fn forget(&mut self, ino: u64, nlookup: u64, open: bool) {
        if ino == 1 {
            return;
        }
        if let Some(entry) = self.entries.get_mut(&ino) {
            entry.lookups = entry.lookups.saturating_sub(nlookup);
            if entry.lookups == 0 && !open {
                self.entries.remove(&ino);
            }
        }
    }

    pub(crate) fn release_if_unreferenced(&mut self, ino: u64, open: bool) {
        if ino != 1 && !open && self.entries.get(&ino).is_some_and(|e| e.lookups == 0) {
            self.entries.remove(&ino);
        }
    }

    pub(crate) fn remove(&mut self, ino: u64) {
        if ino != 1 {
            self.entries.remove(&ino);
        }
    }

    // Allocate all new names before the APFS transaction is committed. After
    // commit the caller can update every cached descendant without failing.
    pub(crate) fn plan_rename_tree(
        &mut self, ino: u64, old: &str, new: &str,
    ) -> Result<Vec<(u64, String)>> {
        if !self.entries.contains_key(&ino) {
            self.entries.try_reserve(1)
                .map_err(|_| std::io::Error::from_raw_os_error(libc::ENOMEM))?;
        }
        let prefix = format!("{old}/");
        let mut updates = Vec::new();
        updates.try_reserve(self.entries.len() + 1)
            .map_err(|_| std::io::Error::from_raw_os_error(libc::ENOMEM))?;
        updates.push((ino, new.to_owned()));
        for (&id, entry) in &self.entries {
            if id != ino {
                if let Some(suffix) = entry.path.strip_prefix(&prefix) {
                    updates.push((id, format!("{new}/{suffix}")));
                }
            }
        }
        Ok(updates)
    }

    pub(crate) fn apply_rename_tree(&mut self, updates: Vec<(u64, String)>) {
        for (ino, path) in updates {
            if let Some(entry) = self.entries.get_mut(&ino) {
                entry.path = path;
            } else {
                self.entries.insert(ino, Entry { path, lookups: 0 });
            }
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_directory_rewrites_cached_descendant_paths() {
        let mut paths = InodePaths::new();
        paths.remember(2, "/src".into()).unwrap();
        paths.remember(3, "/src/deep".into()).unwrap();
        paths.remember(4, "/src/deep/file".into()).unwrap();
        paths.remember(5, "/src-sibling".into()).unwrap();
        let plan = paths.plan_rename_tree(2, "/src", "/dst/src").unwrap();
        paths.apply_rename_tree(plan);
        assert_eq!(paths.get(2), Some("/dst/src"));
        assert_eq!(paths.get(3), Some("/dst/src/deep"));
        assert_eq!(paths.get(4), Some("/dst/src/deep/file"));
        assert_eq!(paths.get(5), Some("/src-sibling"));
    }

    #[test]
    fn more_than_old_hundred_thousand_inode_limit_and_forget() {
        let mut paths = InodePaths::new();
        for i in 2..=125_001 {
            paths.remember(i, format!("/file-{i}")).unwrap();
        }
        assert_eq!(paths.len(), 125_001);
        paths
            .remember(125_001, "/second-hardlink".to_owned())
            .unwrap();
        assert_eq!(paths.get(125_001), Some("/file-125001"));
        paths.forget(125_001, 1, false);
        assert!(paths.get(125_001).is_some());
        paths.forget(125_001, 1, true);
        assert!(paths.get(125_001).is_some());
        paths.release_if_unreferenced(125_001, false);
        assert!(paths.get(125_001).is_none());
        for i in 2..125_001 {
            paths.forget(i, 1, false);
        }
        assert_eq!(paths.len(), 1);
    }
}
