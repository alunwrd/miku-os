use crate::vfs::hash::name_hash;
use crate::vfs::types::*;

pub struct PathWalker;

impl PathWalker {
    pub fn resolve(
        nodes: &[crate::vfs::vnode::VNode; MAX_VNODES],
        cwd: usize,
        path: &str,
    ) -> VfsResult<usize> {
        let path = path.trim();
        if path.is_empty() {
            return Ok(cwd);
        }

        let mut current = if path.starts_with('/') { 0 } else { cwd };
        let mut depth = 0u8;

        for component in path.split('/') {
            if component.is_empty() || component == "." {
                continue;
            }
            if component == ".." {
                let p = nodes[current].parent;
                if p != INVALID_ID {
                    current = p as usize;
                }
                continue;
            }

            depth += 1;
            if depth as usize > MAX_PATH_DEPTH {
                return Err(VfsError::InvalidPath);
            }

            if !nodes[current].is_dir() {
                return Err(VfsError::NotDirectory);
            }

            let eff = Self::effective_node(nodes, current);
            match Self::lookup_child(nodes, eff, component) {
                Ok(id) => {
                    current = id;
                    if nodes[current].is_symlink() {
                        current = Self::follow_symlink(nodes, current, 0)?;
                    }
                }
                Err(VfsError::NotFound) => return Err(VfsError::NotFound),
                Err(e) => return Err(e),
            }
        }
        Ok(current)
    }

    fn follow_symlink(
        nodes: &[crate::vfs::vnode::VNode; MAX_VNODES],
        link_id: usize,
        depth: usize,
    ) -> VfsResult<usize> {
        if depth >= MAX_SYMLINK_DEPTH {
            return Err(VfsError::TooManySymlinks);
        }
        if !nodes[link_id].is_symlink() {
            return Ok(link_id);
        }

        let target = nodes[link_id].symlink_target.as_str();
        if target.is_empty() {
            return Err(VfsError::InvalidPath);
        }

        let parent = nodes[link_id].parent as usize;
        let start = if target.starts_with('/') { 0 } else { parent };

        let mut current = start;
        for component in target.split('/') {
            if component.is_empty() || component == "." {
                continue;
            }
            if component == ".." {
                let p = nodes[current].parent;
                if p != INVALID_ID {
                    current = p as usize;
                }
                continue;
            }
            if !nodes[current].is_dir() {
                return Err(VfsError::NotDirectory);
            }
            let eff = Self::effective_node(nodes, current);
            current = Self::lookup_child(nodes, eff, component)?;
            if nodes[current].is_symlink() {
                current = Self::follow_symlink(nodes, current, depth + 1)?;
            }
        }
        Ok(current)
    }

    fn lookup_child(
        nodes: &[crate::vfs::vnode::VNode; MAX_VNODES],
        parent: usize,
        name: &str,
    ) -> VfsResult<usize> {
        if let Some(id) = nodes[parent].children.find_by_name(name) {
            let cid = id as usize;
            if cid < MAX_VNODES && nodes[cid].active {
                return Ok(cid);
            }
        }
        Err(VfsError::NotFound)
    }

    #[inline]
    pub fn effective_node(_nodes: &[crate::vfs::vnode::VNode; MAX_VNODES], id: usize) -> usize {
        id
    }

    pub fn split_last(path: &str) -> (&str, &str) {
        let trimmed = path.trim_end_matches('/');
        if trimmed.is_empty() {
            return ("/", "");
        }
        match trimmed.rfind('/') {
            Some(pos) => {
                let dir = if pos == 0 { "/" } else { &trimmed[..pos] };
                (dir, &trimmed[pos + 1..])
            }
            None => (".", trimmed),
        }
    }
}

pub fn split_parent_name(path: &str) -> (&str, &str) {
    let trimmed = path.trim_start_matches('/').trim_end_matches('/');
    if trimmed.is_empty() {
        return ("/", "");
    }
    match trimmed.rfind('/') {
        Some(pos) => {
            let parent = &trimmed[..pos];
            let name = &trimmed[pos + 1..];
            if parent.is_empty() { ("/", name) } else { (parent, name) }
        }
        None => ("/", trimmed),
    }
}
