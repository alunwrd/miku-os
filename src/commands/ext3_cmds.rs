use crate::commands::ext_cmds_common as common;
use crate::commands::ext2_cmds::with_ext2_pub;
use crate::miku_extfs::ext3::journal::DEFAULT_JOURNAL_BLOCKS;
use crate::miku_extfs::FsError;
use crate::{cprintln, print_error, print_success, println};

pub fn cmd_ext3_mount(args: &str) {
    crate::commands::ext2_cmds::cmd_ext2_mount(args);
}

pub fn cmd_ext3_ls(path: &str)                  { common::impl_ls(path, "ext3"); }
pub fn cmd_ext3_cat(path: &str)                 { common::impl_cat(path, "ext3"); }
pub fn cmd_ext3_stat(path: &str)                { common::impl_stat(path, "ext3"); }
pub fn cmd_ext3_write(path: &str, text: &str)   { common::impl_write(path, text, "ext3"); }
pub fn cmd_ext3_mkdir(path: &str)               { common::impl_mkdir(path, "ext3"); }
pub fn cmd_ext3_rm(path: &str)                  { common::impl_rm(path, "ext3"); }
pub fn cmd_ext3_rmdir(path: &str)               { common::impl_rmdir(path, "ext3"); }
pub fn cmd_ext3_append(path: &str, text: &str)  { common::impl_append(path, text, "ext3"); }
pub fn cmd_ext3_tree(path: &str)                { common::impl_tree(path, "ext3"); }
pub fn cmd_ext3_du(path: &str)                  { common::impl_du(path, "ext3"); }

pub fn cmd_ext3_info() {
    let result = with_ext2_pub(|fs| fs.scan_journal());
    match result {
        Some(Ok(info)) => {
            if !info.valid { print_error!("  no journal found"); return; }
            cprintln!(57, 197, 187, "  ext3 Journal Info");
            println!("  Version:    {}", if info.version == 2 { "JBD2" } else { "JBD1" });
            println!("  Block size: {} bytes", info.block_size);
            println!("  Total:      {} blocks", info.total_blocks);
            println!("  Size:       {} KB", info.journal_size / 1024);
            println!("  First:      block {}", info.first_block);
            println!("  Sequence:   {}", info.sequence);
            println!("  Start:      {}", info.start);
            if info.clean { print_success!("  Status:     clean"); }
            else { print_error!("  Status:     dirty ({} transactions)", info.transaction_count); }
            if info.errno != 0 { print_error!("  Errno:      {}", info.errno); }
        }
        Some(Err(FsError::NoJournal)) => print_error!("  no journal"),
        Some(Err(e)) => print_error!("  ext3info: {:?}", e),
        None => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_ext3_journal() {
    let result = with_ext2_pub(|fs| fs.scan_journal());
    match result {
        Some(Ok(info)) => {
            if !info.valid { print_error!("  no journal found"); return; }
            if info.clean { print_success!("  journal is clean"); return; }
            cprintln!(57, 197, 187, "  Journal Transactions ({}):", info.transaction_count);
            for i in 0..info.transaction_count {
                let tx = &info.transactions[i];
                if !tx.active { continue; }
                if tx.committed {
                    cprintln!(100, 220, 150, "  {:>6}  {:>8}  {:>6}  committed",
                        tx.sequence, tx.start_block, tx.data_blocks);
                } else {
                    cprintln!(255, 50, 50, "  {:>6}  {:>8}  {:>6}  incomplete",
                        tx.sequence, tx.start_block, tx.data_blocks);
                }
            }
        }
        Some(Err(FsError::NoJournal)) => print_error!("  no journal"),
        Some(Err(e)) => print_error!("  ext3journal: {:?}", e),
        None => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_ext3_mkjournal() {
    let result = with_ext2_pub(|fs| -> Result<(), FsError> {
        fs.ext3_create_journal(DEFAULT_JOURNAL_BLOCKS)
    });
    match result {
        Some(Ok(())) => print_success!("  ext3 journal created ({} blocks)", DEFAULT_JOURNAL_BLOCKS),
        Some(Err(FsError::AlreadyExists)) => print_error!("  journal already exists"),
        Some(Err(e)) => print_error!("  ext3mkjournal: {:?}", e),
        None => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_ext3_clean() {
    let result = with_ext2_pub(|fs| fs.ext3_clean_journal());
    match result {
        Some(Ok(())) => print_success!("  journal marked clean"),
        Some(Err(FsError::NoJournal)) => print_error!("  no journal found"),
        Some(Err(e)) => print_error!("  ext3clean: {:?}", e),
        None => print_error!("  ext2 not mounted"),
    }
}

pub fn cmd_ext3_recover() {
    let result = with_ext2_pub(|fs| fs.ext3_recover());
    match result {
        Some(Ok(0)) => print_success!("  no recovery needed"),
        Some(Ok(n)) => print_success!("  recovered {} blocks", n),
        Some(Err(FsError::NoJournal)) => print_error!("  no journal found"),
        Some(Err(e)) => print_error!("  ext3recover: {:?}", e),
        None => print_error!("  ext2 not mounted"),
    }
}
