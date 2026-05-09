use crate::miku_extfs::structs::*;
use crate::miku_extfs::FsError;
use crate::{cprintln, print_error, print_success, println};
use crate::commands::ext2_cmds::{with_ext2_pub, is_ext2_ready};
use crate::commands::ext_cmds_common as common;
use crate::commands::ext_cmds_common::resolve_parent_and_name;

#[inline(always)]
fn yn(b: bool) -> &'static str { if b { "yes" } else { "no" } }

pub fn cmd_ext4_mount(args: &str) {
    crate::commands::ext2_cmds::cmd_ext2_mount(args);
    if !is_ext2_ready() { return; }

    let _ = with_ext2_pub(|fs| {
        let sb = &fs.superblock;

        cprintln!(57, 197, 187, "  ext4 Feature Report");

        let has_journal   = sb.has_journal();
        let has_dir_index = sb.has_dir_index();
        let has_ext_attr  = sb.has_ext_attr();
        println!("  compat:    journal={} dir_index={} ext_attr={}",
            yn(has_journal), yn(has_dir_index), yn(has_ext_attr));

        let has_extents  = sb.has_extents();
        let has_filetype = sb.has_filetype();
        let has_64bit    = sb.has_64bit();
        let has_flex_bg  = sb.has_flex_bg();
        println!("  incompat:  extents={} filetype={} 64bit={} flex_bg={}",
            yn(has_extents), yn(has_filetype), yn(has_64bit), yn(has_flex_bg));

        let has_sparse = sb.has_sparse_super();
        let has_large  = sb.has_large_file();
        let has_huge   = sb.has_huge_file();
        let has_nlink  = sb.feature_ro_compat() & FEATURE_RO_COMPAT_DIR_NLINK != 0;
        let has_eisize = sb.feature_ro_compat() & FEATURE_RO_COMPAT_EXTRA_ISIZE != 0;
        let has_csum   = sb.has_metadata_csum();
        println!("  ro_compat: sparse={} large_file={} huge={} dir_nlink={} extra_isize={} metadata_csum={}",
            yn(has_sparse), yn(has_large), yn(has_huge), yn(has_nlink), yn(has_eisize), yn(has_csum));

        println!("  inode size: {} bytes  rev_level: {}", fs.inode_size(), sb.rev_level());

        if fs.ext4_features_complete() {
            print_success!("  All mandatory ext4 features present.");
        } else {
            let (mi, mr) = fs.ext4_missing_features();
            crate::print_warn!("  warning: this is NOT a complete ext4 filesystem.");
            if mi & FEATURE_INCOMPAT_EXTENTS  != 0 { crate::print_warn!("    missing: INCOMPAT_EXTENTS"); }
            if mi & FEATURE_INCOMPAT_FILETYPE != 0 { crate::print_warn!("    missing: INCOMPAT_FILETYPE"); }
            if mr & FEATURE_RO_COMPAT_SPARSE_SUPER != 0 { crate::print_warn!("    missing: RO_SPARSE_SUPER"); }
            if mr & FEATURE_RO_COMPAT_LARGE_FILE   != 0 { crate::print_warn!("    missing: RO_LARGE_FILE"); }
            if mr & FEATURE_RO_COMPAT_DIR_NLINK    != 0 { crate::print_warn!("    missing: RO_DIR_NLINK"); }
            crate::print_warn!("  Run 'ext4upgrade' to fix, then remount.");
        }

        if has_journal {
            if let Ok(info) = fs.scan_journal() {
                if info.clean { print_success!("  Journal: active, clean ({} blocks)", info.total_blocks); }
                else          { crate::print_warn!("  Journal: dirty - run ext3recover"); }
            }
        } else {
            crate::print_warn!("  Journal: none (run ext3mkjournal + remount for ext3/ext4)");
        }
    });
}

pub fn cmd_ext4_upgrade() {
    let result = with_ext2_pub(|fs| -> Result<crate::miku_extfs::ext4::upgrade::Ext4UpgradeReport, FsError> {
        fs.ext4_upgrade()
    });
    match result {
        None             => { print_error!("  not mounted (run ext2mount / ext4mount first)"); }
        Some(Err(e))     => { print_error!("  ext4upgrade: {:?}", e); }
        Some(Ok(rep))    => {
            if rep.already_ext4 && !rep.any_new() {
                print_success!("  filesystem is already fully ext4 - nothing changed.");
                return;
            }
            cprintln!(57, 197, 187, "  ext4 upgrade");
            if rep.set_rev_level    { print_success!("  rev_level bumped to 1 (EXT2_DYNAMIC_REV)"); }
            if rep.set_extents      { print_success!("  FEATURE_INCOMPAT_EXTENTS         enabled"); }
            if rep.set_filetype     { print_success!("  FEATURE_INCOMPAT_FILETYPE        enabled"); }
            if rep.set_sparse_super { print_success!("  FEATURE_RO_COMPAT_SPARSE_SUPER   enabled"); }
            if rep.set_large_file   { print_success!("  FEATURE_RO_COMPAT_LARGE_FILE     enabled"); }
            if rep.set_dir_nlink    { print_success!("  FEATURE_RO_COMPAT_DIR_NLINK      enabled"); }
            if rep.set_extra_isize  { print_success!("  FEATURE_RO_COMPAT_EXTRA_ISIZE    enabled"); }
            if rep.set_dir_index    { print_success!("  FEATURE_COMPAT_DIR_INDEX         enabled"); }
            if !rep.had_journal {
                crate::print_warn!("  note: no journal - run ext3mkjournal for full ext3/ext4 safety");
            }
            if rep.inode_size_warning {
                crate::print_warn!("  inode_size = {} bytes (< 256)", rep.inode_size);
                crate::print_warn!("  EXTRA_ISIZE requires 256-byte inodes (mkfs.ext4 -I 256)");
            }
            if rep.any_new() {
                print_success!("  Superblock written.  Remount with ext4mount to verify.");
            }
        }
    }
}

pub fn cmd_ext4_ls(path: &str)                  { common::impl_ls(path, "ext4"); }
pub fn cmd_ext4_cat(path: &str)                 { common::impl_cat(path, "ext4"); }
pub fn cmd_ext4_stat(path: &str)                { common::impl_stat(path, "ext4"); }
pub fn cmd_ext4_write(path: &str, text: &str)   { common::impl_write(path, text, "ext4"); }
pub fn cmd_ext4_mkdir(path: &str)               { common::impl_mkdir(path, "ext4"); }
pub fn cmd_ext4_rm(path: &str)                  { common::impl_rm(path, "ext4"); }
pub fn cmd_ext4_rmdir(path: &str)               { common::impl_rmdir(path, "ext4"); }
pub fn cmd_ext4_append(path: &str, text: &str)  { common::impl_append(path, text, "ext4"); }
pub fn cmd_ext4_tree(path: &str)                { common::impl_tree(path, "ext4"); }
pub fn cmd_ext4_du(path: &str)                  { common::impl_du(path, "ext4"); }
pub fn cmd_ext4_cp(src: &str, dst: &str)        { common::impl_cp(src, dst, "ext4"); }
pub fn cmd_ext4_sync()                          { common::impl_sync("ext4"); }

pub fn cmd_ext4_extinfo(path: &str)             { cmd_ext4_extent_info(path); }

pub fn cmd_ext4_info() {
    let result = with_ext2_pub(|fs| {
        let info = fs.fs_info();
        cprintln!(57, 197, 187, "  ext4 Filesystem Info");
        println!("  Version:   {}", info.version);
        println!("  Extents:   {}", if info.has_extents { "enabled" } else { "disabled" });
        println!("  Journal:   {}", if info.has_journal { "enabled" } else { "disabled" });
        println!("  64bit:     {}", if fs.superblock.has_64bit() { "yes" } else { "no" });
        println!("  Checksums: {}", if fs.superblock.has_metadata_csum() { "crc32c" } else { "none" });
        println!("  Flex BG:   {}", if fs.superblock.has_flex_bg() { "yes" } else { "no" });
        if fs.superblock.has_metadata_csum() {
            if fs.verify_superblock_csum() { print_success!("  SB csum:   valid"); }
            else { print_error!("  SB csum:   invalid"); }
        }
    });
    if result.is_none() { print_error!("  ext2 not mounted"); }
}

pub fn cmd_ext4_enable_extents() {
    let result = with_ext2_pub(|fs| -> Result<(), FsError> { fs.enable_extents_feature() });
    match result {
        Some(Ok(())) => print_success!("  extents enabled"),
        Some(Err(e)) => print_error!("  ext4extents: {:?}", e),
        None         => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_ext4_checksums() {
    let result = with_ext2_pub(|fs| {
        cprintln!(57, 197, 187, "  Checksum Verification");
        let sb_ok = fs.verify_superblock_csum();
        println!("  Superblock: {}", if sb_ok { "ok" } else { "fail" });
        let gc = fs.group_count as usize;
        let (mut gd_ok, mut gd_fail) = (0u32, 0u32);
        let (mut bb_ok, mut bb_fail) = (0u32, 0u32);
        let (mut ib_ok, mut ib_fail) = (0u32, 0u32);
        for g in 0..gc.min(32) {
            if fs.verify_group_desc_csum(g) { gd_ok += 1; } else { gd_fail += 1; }
            if fs.verify_block_bitmap_csum(g) { bb_ok += 1; } else { bb_fail += 1; }
            if fs.verify_inode_bitmap_csum(g) { ib_ok += 1; } else { ib_fail += 1; }
        }
        println!("  Group descs:   {} ok, {} fail", gd_ok, gd_fail);
        println!("  Block bitmaps: {} ok, {} fail", bb_ok, bb_fail);
        println!("  Inode bitmaps: {} ok, {} fail", ib_ok, ib_fail);
        let (mut ino_ok, mut ino_fail) = (0u32, 0u32);
        let max_check = fs.superblock.inodes_count().min(64);
        for ino in 1..=max_check {
            if let Ok(inode) = fs.read_inode(ino) {
                if inode.mode() != 0 {
                    if fs.verify_inode_csum(ino, &inode) { ino_ok += 1; } else { ino_fail += 1; }
                }
            }
        }
        println!("  Inodes (first {}): {} ok, {} fail", max_check, ino_ok, ino_fail);
    });
    if result.is_none() { print_error!("  ext2 not mounted"); }
}

pub fn cmd_ext4_extent_info(path: &str) {
    if path.is_empty() { println!("Usage: ext4extinfo <path>"); return; }
    let result = with_ext2_pub(|fs| -> Result<(), FsError> {
        let ino = fs.resolve_path(path)?;
        let inode = fs.read_inode(ino)?;
        if !inode.uses_extents() {
            println!("  inode {} does not use extents (indirect blocks)", ino);
            return Ok(());
        }
        let header = inode.extent_header();
        println!("  Inode: {}", ino);
        println!("  Extent tree depth: {}", header.depth);
        println!("  Entries: {} / {}", header.entries, header.max);
        let count = fs.ext4_extent_count(&inode)?;
        println!("  Total extents: {}", count);
        if header.depth == 0 {
            for i in 0..header.entries as usize {
                let ext = inode.extent_at(i);
                println!("    [{}] logical={} len={} phys={}",
                    i, ext.block, ext.actual_len(), ext.start());
            }
        }
        Ok(())
    });
    match result {
        Some(Ok(())) => {}
        Some(Err(e)) => print_error!("  ext4extinfo: {:?}", e),
        None         => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_fiemap(path: &str) {
    if path.is_empty() { println!("Usage: fiemap <path>"); return; }
    let result = with_ext2_pub(|fs| -> Result<(), FsError> {
        let ino = fs.resolve_path(path)?;
        let mut extents = [(0u32, 0u32, 0u32); 64];
        let count = fs.ext4_fiemap(ino, &mut extents)?;
        println!("  File extent map for inode {}:", ino);
        if count == 0 {
            println!("  (no extents / empty file)");
        }
        let mut total_blocks = 0u32;
        for i in 0..count {
            let (logical, phys, len) = extents[i];
            println!("    [{:2}] logical={:<6} phys={:<8} len={}", i, logical, phys, len);
            total_blocks += len;
        }
        println!("  {} extents, {} blocks total", count, total_blocks);
        Ok(())
    });
    match result {
        Some(Ok(())) => {}
        Some(Err(e)) => print_error!("  fiemap: {:?}", e),
        None         => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_ext4_fsck() {
    let result = with_ext2_pub(|fs| fs.ext2_fsck());
    match result {
        Some(r) => {
            if !r.checked { print_error!("  fsck failed"); return; }
            cprintln!(57, 197, 187, "  ext4 filesystem check");
            println!("  Blocks: {} / {} free", r.free_blocks, r.total_blocks);
            println!("  Inodes: {} used / {} total", r.used_inodes, r.total_inodes);
            if r.errors == 0 { print_success!("  filesystem ok"); }
            else             { print_error!("  {} errors found", r.errors); }
        }
        None => print_error!("  not mounted"),
    }
}
