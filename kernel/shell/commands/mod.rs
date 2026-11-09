pub mod ext2_cmds;
pub mod ext_cmds_common;
pub mod ext3_cmds;
pub mod ext4_cmds;
pub mod xattr_cmds;
pub mod fs;
pub mod system;
pub mod nvidia_cmds;
pub mod mkfs_cmds;
pub mod disk_cmds;
pub mod install_cmds;
pub mod mayland_cmds;
pub mod display_cmds;
pub mod elf_cmds;

extern crate alloc;

use crate::{println, serial_println};

macro_rules! ext_dispatch {
    ($fn2:expr, $fn3:expr, $fn4:expr) => {
        match fs::ext_version() {
            "ext4" => $fn4,
            "ext3" => $fn3,
            _      => $fn2,
        }
    };
}

/// Runs a program the person at the console asked for.
///
/// `exec_as`, not `exec`: a program the user started should be the user. The
/// credential comes from the shell thread, which `login` set, so `cat` and
/// `/bin/cat` answer the same way and `mikudo` in front of either means the
/// same.
///
/// Machine commands stay ring 0: somebody who can type at this console can
/// power-cycle it and boot something else. What is enforced is everything the
/// VFS mediates, which is where the secrets are
fn cmd_exec(path: &str, args: &[&str]) {
    let cred = crate::sched::current_cred();
    let env = user_env(cred.uid);
    let env: alloc::vec::Vec<&str> = env.iter().map(|s| s.as_str()).collect();
    match crate::process::exec::exec_as(path, args, &env, cred) {
        Ok(pid) => {
            // The keyboard goes to the program the user is waiting on, and
            // comes back when it exits. `exec` does not do this: a daemon
            // started by mikuD must never take the console
            crate::io::input::user_stdin::set_foreground(pid);
            crate::sched::waitpid(pid);
            crate::io::input::user_stdin::clear_foreground();
        }
        Err(e) => {
            crate::print_error!("  exec: {}", e.as_str());
        }
    }
}

/// The environment of a program started from this shell: the one a login
/// session gets from `mikudod` (`session_env`), for whoever this shell is.
/// `SHELL` is set because the passwd field names a session program here, not a
/// shell
fn user_env(uid: u16) -> alloc::vec::Vec<alloc::string::String> {
    use alloc::format;
    let (name, home) = crate::syscall::adapters::process::passwd_name_home(uid)
        .unwrap_or_else(|| (alloc::string::String::new(), "/".into()));
    let mut env = alloc::vec![
        format!("PATH=/bin"),
        format!("HOME={home}"),
        format!("SHELL=/bin/msh"),
        format!("TERM=miku"),
        format!("OS=MikuOS"),
    ];
    if !name.is_empty() {
        env.push(format!("USER={name}"));
        env.push(format!("LOGNAME={name}"));
    }
    env
}

/// Runs one command line. False means nothing here answers to that name
///
/// `kcmd` turns that into `-ENOENT`, so a ring-3 shell that asked because it
/// could not find a program can print its own "not found" and exit 127, and
/// `cmd || echo failed` works
pub fn execute(input: &str) -> bool {
    let t = input.trim();
    if t.is_empty() { return true; }

    let mut parts = t.split_whitespace();
    let cmd  = parts.next().unwrap_or("");
    let a1   = parts.next().unwrap_or("");
    let a2   = parts.next().unwrap_or("");
    let a3   = parts.next().unwrap_or("");
    let rest = if t.len() > cmd.len() { t[cmd.len()..].trim_start() } else { "" };

    // Two different questions. Powering off is `CAP_SYS_BOOT`, which a session
    // holds: somebody at this seat can reach the button on the case. Everything
    // else on the list is `CAP_SYS_ADMIN` (mounting a disk, partitioning one,
    // starting services), and that is what mikudo is for
    if let Some(cap) = gated_by(cmd, rest) {
        if !crate::sched::current_cred().has(cap) {
            crate::print_error!("  {}: this is a machine command and you are not root", cmd);
            crate::println!("  it changes the machine, so it wants CAP_SYS_ADMIN, which a");
            crate::println!("  desktop session deliberately does not carry. Ask for it:");
            crate::println!("     mikudo {} ...", cmd);
            // Refused, but answered: the name exists and this is what it said
            return true;
        }
    }

    // The everyday commands are not here. `ls`, `cat`, `cp`, `mv`, `rm`,
    // `mkdir`, `rmdir`, `ln`, `readlink`, `touch`, `stat`, `chmod`, `chown`,
    // `du`, `df`, `tree`, `head`, `tail`, `wc`, `grep`, `sort`, `uniq`, `tee`,
    // `echo`, `ps`, `kill`, `sleep`, `date`, `uptime`, `uname`, `free`,
    // `clear`, `which` and `env` are programs in /bin. An unknown name falls
    // through to `run_external` at the bottom of this match. See utils/ and
    // dev/terminal-architecture.md.
    //
    // `cd` and `pwd` stay, because they are the shell's own state: a `cd` in a
    // child would change nothing anybody sees.
    match cmd {
        "cd"  => fs::cmd_cd(a1),
        "pwd" => fs::cmd_pwd(),

        "write" => {
            if a1.is_empty() || rest.len() <= a1.len() { println!("Usage: write <file> <text>"); }
            else { fs::cmd_write(a1, rest[a1.len()..].trim_start()); }
        }
        "dd" => fs::cmd_dd(rest),
        "mount"  => { if a1.is_empty() { fs::cmd_mount_list(); } else { fs::cmd_mount(a1, a2); } }
        "umount" => { if a1.is_empty() { println!("Usage: umount <path>"); } else { fs::cmd_umount(a1); } }

        "ext2mount" => ext2_cmds::cmd_ext2_mount(rest),
        "ext3mount" => ext3_cmds::cmd_ext3_mount(rest),
        "ext4mount" => ext4_cmds::cmd_ext4_mount(rest),

        "fs.list"   => ext2_cmds::cmd_fs_list(),
        "fs.select" => ext2_cmds::cmd_fs_select(rest),
        "fs.rw"     => ext2_cmds::cmd_fs_remount(rest, true),
        "fs.ro"     => ext2_cmds::cmd_fs_remount(rest, false),
        "fs.umount" => ext2_cmds::cmd_fs_umount(rest),

        "extls" => fs::cmd_extls(a1),

        "extcat" => {
            if a1.is_empty() { println!("Usage: extcat <path>"); }
            else { ext_dispatch!(
                ext2_cmds::cmd_ext2_cat(a1),
                ext3_cmds::cmd_ext3_cat(a1),
                ext4_cmds::cmd_ext4_cat(a1)
            ); }
        }
        "extstat" => {
            if a1.is_empty() { println!("Usage: extstat <path>"); }
            else { ext_dispatch!(
                ext2_cmds::cmd_ext2_stat(a1),
                ext3_cmds::cmd_ext3_stat(a1),
                ext4_cmds::cmd_ext4_stat(a1)
            ); }
        }
        "extinfo" => {
            ext_dispatch!(
                ext2_cmds::cmd_ext2_info(),
                ext3_cmds::cmd_ext3_info(),
                ext4_cmds::cmd_ext4_info()
            );
        }
        "extwrite" => {
            if a1.is_empty() || rest.len() <= a1.len() { println!("Usage: extwrite <path> <text>"); }
            else {
                let text = rest[a1.len()..].trim_start();
                fs::cmd_extwrite(a1, text);
            }
        }
        "extappend" => {
            if a1.is_empty() || rest.len() <= a1.len() { println!("Usage: extappend <path> <text>"); }
            else {
                let text = rest[a1.len()..].trim_start();
                ext_dispatch!(
                    ext2_cmds::cmd_ext2_append(a1, text),
                    ext3_cmds::cmd_ext3_append(a1, text),
                    ext4_cmds::cmd_ext4_append(a1, text)
                );
            }
        }
        "exttouch" => {
            if a1.is_empty() { println!("Usage: exttouch <path>"); }
            else { fs::cmd_exttouch(a1); }
        }
        "extmkdir" => {
            if a1.is_empty() { println!("Usage: extmkdir <path>"); }
            else { fs::cmd_extmkdir(a1); }
        }
        "extrm" => {
            if a1.is_empty() { println!("Usage: extrm [-rf] <path>"); }
            else if a1 == "-rf" || a1 == "-r" {
                if a2.is_empty() { println!("Usage: extrm -rf <path>"); }
                else {
                    let abs = fs::make_abs_path_pub(a2);
                    let abs_str = unsafe { core::str::from_utf8_unchecked(&abs.0[..abs.1]) };
                    ext_dispatch!(
                        ext2_cmds::cmd_ext2_rm_rf(abs_str),
                        ext3_cmds::cmd_ext3_rm(abs_str),
                        ext4_cmds::cmd_ext4_rm(abs_str)
                    );
                }
            } else {
                let abs = fs::make_abs_path_pub(a1);
                let abs_str = unsafe { core::str::from_utf8_unchecked(&abs.0[..abs.1]) };
                ext_dispatch!(
                    ext2_cmds::cmd_ext2_rm(abs_str),
                    ext3_cmds::cmd_ext3_rm(abs_str),
                    ext4_cmds::cmd_ext4_rm(abs_str)
                );
            }
        }
        "extrmdir" => {
            if a1.is_empty() { println!("Usage: extrmdir <path>"); }
            else {
                // To the ext implementation, like every other `ext*` command
                let abs = fs::make_abs_path_pub(a1);
                let abs_str = unsafe { core::str::from_utf8_unchecked(&abs.0[..abs.1]) };
                ext_dispatch!(
                    ext2_cmds::cmd_ext2_rmdir(abs_str),
                    ext3_cmds::cmd_ext3_rmdir(abs_str),
                    ext4_cmds::cmd_ext4_rmdir(abs_str)
                );
            }
        }
        "extmv" => {
            if a1.is_empty() || a2.is_empty() { println!("Usage: extmv <path> <newname>"); }
            else { fs::cmd_extmv(a1, a2); }
        }
        "extcp" => {
            if a1.is_empty() || a2.is_empty() { println!("Usage: extcp <src> <dst>"); }
            else { fs::cmd_extcp(a1, a2); }
        }
        "extln" => {
            if a1 == "-s" {
                if a2.is_empty() || a3.is_empty() { println!("Usage: extln -s <target> <linkname>"); }
                else {
                    ext_dispatch!(
                        ext2_cmds::cmd_ext2_symlink(a2, a3),
                        ext2_cmds::cmd_ext2_symlink(a2, a3),
                        ext2_cmds::cmd_ext2_symlink(a2, a3)
                    );
                }
            } else {
                println!("Usage: extln -s <target> <linkname>");
            }
        }
        "extlink" => {
            if a1.is_empty() || a2.is_empty() { println!("Usage: extlink <existing> <linkname>"); }
            else {
                ext_dispatch!(
                    ext2_cmds::cmd_ext2_hardlink(a1, a2),
                    ext2_cmds::cmd_ext2_hardlink(a1, a2),
                    ext2_cmds::cmd_ext2_hardlink(a1, a2)
                );
            }
        }
        "extchmod" => {
            if a1.is_empty() || a2.is_empty() { println!("Usage: extchmod <mode> <path>"); }
            else { fs::cmd_extchmod(a1, a2); }
        }
        "extchown" => {
            if a1.is_empty() || a2.is_empty() || a3.is_empty() {
                println!("Usage: extchown <uid> <gid> <path>");
            } else { fs::cmd_extchown(a1, a2, a3); }
        }
        "extdu"   => { ext_dispatch!(
            ext2_cmds::cmd_ext2_du(a1),
            ext3_cmds::cmd_ext3_du(a1),
            ext4_cmds::cmd_ext4_du(a1)
        ); }
        "exttree" => { ext_dispatch!(
            ext2_cmds::cmd_ext2_tree(a1),
            ext3_cmds::cmd_ext3_tree(a1),
            ext4_cmds::cmd_ext4_tree(a1)
        ); }
        "extfsck" => ext_cmds_common::impl_fsck(a1 == "-y" || a1 == "--repair"),
        "extcache"      => ext2_cmds::cmd_ext2_cache(),
        "extcacheflush" => ext2_cmds::cmd_ext2_cache_flush(),
        "extsync" | "sync" => ext4_cmds::cmd_ext4_sync(),

        "ext3mkjournal" => ext3_cmds::cmd_ext3_mkjournal(),
        "ext3journal"   => ext3_cmds::cmd_ext3_journal(),
        "ext3recover"   => ext3_cmds::cmd_ext3_recover(),
        "ext3clean"     => ext3_cmds::cmd_ext3_clean(),

        "ext4extents"   => ext4_cmds::cmd_ext4_enable_extents(),
        "ext4checksums" => ext4_cmds::cmd_ext4_checksums(),
        "ext4extinfo"   => {
            if a1.is_empty() { println!("Usage: ext4extinfo <path>"); }
            else { ext4_cmds::cmd_ext4_extent_info(a1); }
        }

        "ext2ls"     => ext2_cmds::cmd_ext2_ls(a1),
        "ext2cat"    => ext2_cmds::cmd_ext2_cat(a1),
        "ext2stat"   => ext2_cmds::cmd_ext2_stat(a1),
        "ext2info"   => ext2_cmds::cmd_ext2_info(),
        "ext2write"  => {
            let text = if rest.len() > a1.len() { rest[a1.len()..].trim_start() } else { "" };
            if a1.is_empty() { println!("Usage: ext2write <path> <text>"); }
            else { ext2_cmds::cmd_ext2_write(a1, text); }
        }
        "ext2append" => {
            let text = if rest.len() > a1.len() { rest[a1.len()..].trim_start() } else { "" };
            if a1.is_empty() { println!("Usage: ext2append <path> <text>"); }
            else { ext2_cmds::cmd_ext2_append(a1, text); }
        }
        "ext2touch"  => { if a1.is_empty() { println!("Usage: ext2touch <path>"); } else { fs::cmd_exttouch(a1); } }
        "ext2mkdir"  => { if a1.is_empty() { println!("Usage: ext2mkdir <path>"); } else { ext2_cmds::cmd_ext2_mkdir(a1); } }
        "ext2rm"     => {
            if a1.is_empty() { println!("Usage: ext2rm [-rf] <path>"); }
            else if a1 == "-rf" || a1 == "-r" {
                if a2.is_empty() { println!("Usage: ext2rm -rf <path>"); }
                else { ext2_cmds::cmd_ext2_rm_rf(a2); }
            } else { ext2_cmds::cmd_ext2_rm(a1); }
        }
        "ext2rmdir"  => { if a1.is_empty() { println!("Usage: ext2rmdir <path>"); } else { ext2_cmds::cmd_ext2_rmdir(a1); } }
        "ext2mv"     => {
            if a1.is_empty() || a2.is_empty() { println!("Usage: ext2mv <path> <newname>"); }
            else { ext2_cmds::cmd_ext2_rename(a1, a2); }
        }
        "ext2cp"     => {
            if a1.is_empty() || a2.is_empty() { println!("Usage: ext2cp <src> <dst>"); }
            else { ext2_cmds::cmd_ext2_cp(a1, a2); }
        }
        "ext2ln"     => {
            if a1 == "-s" {
                if a2.is_empty() || a3.is_empty() { println!("Usage: ext2ln -s <target> <linkname>"); }
                else { ext2_cmds::cmd_ext2_symlink(a2, a3); }
            } else { println!("Usage: ext2ln -s <target> <linkname>"); }
        }
        "ext2link"   => {
            if a1.is_empty() || a2.is_empty() { println!("Usage: ext2link <existing> <linkname>"); }
            else { ext2_cmds::cmd_ext2_hardlink(a1, a2); }
        }
        "ext2chmod"  => {
            if a1.is_empty() || a2.is_empty() { println!("Usage: ext2chmod <mode> <path>"); }
            else { ext2_cmds::cmd_ext2_chmod(a1, a2); }
        }
        "ext2chown"  => {
            if a1.is_empty() || a2.is_empty() || a3.is_empty() { println!("Usage: ext2chown <uid> <gid> <path>"); }
            else { ext2_cmds::cmd_ext2_chown(a1, a2, a3); }
        }
        "ext2du"        => ext2_cmds::cmd_ext2_du(a1),
        "ext2tree"      => ext2_cmds::cmd_ext2_tree(a1),
        "ext2fsck"      => ext2_cmds::cmd_ext2_fsck(a1 == "-y" || a1 == "--repair"),
        "ext2cache"     => ext2_cmds::cmd_ext2_cache(),
        "ext2cacheflush"=> ext2_cmds::cmd_ext2_cache_flush(),

        "ext3ls"     => ext3_cmds::cmd_ext3_ls(a1),
        "ext3cat"    => ext3_cmds::cmd_ext3_cat(a1),
        "ext3stat"   => ext3_cmds::cmd_ext3_stat(a1),
        "ext3info"   => ext3_cmds::cmd_ext3_info(),
        "ext3write"  => {
            let text = if rest.len() > a1.len() { rest[a1.len()..].trim_start() } else { "" };
            if a1.is_empty() { println!("Usage: ext3write <path> <text>"); }
            else { ext3_cmds::cmd_ext3_write(a1, text); }
        }
        "ext3append" => {
            let text = if rest.len() > a1.len() { rest[a1.len()..].trim_start() } else { "" };
            if a1.is_empty() { println!("Usage: ext3append <path> <text>"); }
            else { ext3_cmds::cmd_ext3_append(a1, text); }
        }
        "ext3mkdir"  => { if a1.is_empty() { println!("Usage: ext3mkdir <path>"); } else { ext3_cmds::cmd_ext3_mkdir(a1); } }
        "ext3rm"     => { if a1.is_empty() { println!("Usage: ext3rm <path>"); } else { ext3_cmds::cmd_ext3_rm(a1); } }
        "ext3rmdir"  => { if a1.is_empty() { println!("Usage: ext3rmdir <path>"); } else { ext3_cmds::cmd_ext3_rmdir(a1); } }
        "ext3tree"   => ext3_cmds::cmd_ext3_tree(a1),
        "ext3du"     => ext3_cmds::cmd_ext3_du(a1),

        "ext4ls"     => ext4_cmds::cmd_ext4_ls(a1),
        "ext4cat"    => ext4_cmds::cmd_ext4_cat(a1),
        "ext4stat"   => ext4_cmds::cmd_ext4_stat(a1),
        "ext4info"   => ext4_cmds::cmd_ext4_info(),
        "ext4sync"   => ext4_cmds::cmd_ext4_sync(),
        "ext4write"  => {
            let text = if rest.len() > a1.len() { rest[a1.len()..].trim_start() } else { "" };
            if a1.is_empty() { println!("Usage: ext4write <path> <text>"); }
            else { ext4_cmds::cmd_ext4_write(a1, text); }
        }
        "ext4append" => {
            let text = if rest.len() > a1.len() { rest[a1.len()..].trim_start() } else { "" };
            if a1.is_empty() { println!("Usage: ext4append <path> <text>"); }
            else { ext4_cmds::cmd_ext4_append(a1, text); }
        }
        "ext4mkdir"  => { if a1.is_empty() { println!("Usage: ext4mkdir <path>"); } else { ext4_cmds::cmd_ext4_mkdir(a1); } }
        "ext4rm"     => { if a1.is_empty() { println!("Usage: ext4rm <path>"); } else { ext4_cmds::cmd_ext4_rm(a1); } }
        "ext4rmdir"  => { if a1.is_empty() { println!("Usage: ext4rmdir <path>"); } else { ext4_cmds::cmd_ext4_rmdir(a1); } }
        "ext4cp"     => {
            if a1.is_empty() || a2.is_empty() { println!("Usage: ext4cp <src> <dst>"); }
            else { ext4_cmds::cmd_ext4_cp(a1, a2); }
        }
        "ext4tree"   => ext4_cmds::cmd_ext4_tree(a1),
        "ext4du"     => ext4_cmds::cmd_ext4_du(a1),
        "ext4fsck"   => ext4_cmds::cmd_ext4_fsck(a1 == "-y" || a1 == "--repair"),

        "fiemap"     => ext4_cmds::cmd_fiemap(a1),
        "getxattr"   => { if a1.is_empty() || a2.is_empty() { println!("Usage: getxattr <path> <name>"); } else { xattr_cmds::cmd_getxattr(a1, a2); } }
        "setxattr"   => {
            let val = if rest.len() > a1.len() + a2.len() + 1 {
                rest[a1.len()..].trim_start()[a2.len()..].trim_start()
            } else { "" };
            if a1.is_empty() || a2.is_empty() { println!("Usage: setxattr <path> <name> <value>"); }
            else { xattr_cmds::cmd_setxattr(a1, a2, val); }
        }
        "listxattr"  => xattr_cmds::cmd_listxattr(a1),
        "chattr"     => {
            if a1.is_empty() || a2.is_empty() { println!("Usage: chattr <+/-flags> <path>  (i=immutable, a=append, d=nodump, A=noatime)"); }
            else { xattr_cmds::cmd_chattr(a1, a2); }
        }
        "lsattr"     => xattr_cmds::cmd_lsattr(a1),

        "mkfs.ext2" => {
            if a1.is_empty() { println!("Usage: mkfs.ext2 <drive 0-3>"); }
            else { mkfs_cmds::cmd_mkfs_ext2(rest); }
        }
        "mkfs.ext3" => {
            if a1.is_empty() { println!("Usage: mkfs.ext3 <drive 0-3>"); }
            else { mkfs_cmds::cmd_mkfs_ext3(rest); }
        }
        "mkfs.ext4" => {
            if a1.is_empty() { println!("Usage: mkfs.ext4 <drive 0-3>"); }
            else { mkfs_cmds::cmd_mkfs_ext4(rest); }
        }
        "mkfs.dry"  => {
            if a1.is_empty() || a2.is_empty() { println!("Usage: mkfs.dry <drive 0-3> <ext2|ext3|ext4>"); }
            else { mkfs_cmds::cmd_mkfs_dry(a1, a2); }
        }

        "install"  => install_cmds::cmd_install(rest),

        "gpt"      => disk_cmds::cmd_gpt_show(a1),
        "gpt.init" => disk_cmds::cmd_gpt_init(a1),
        "gpt.add"  => disk_cmds::cmd_gpt_add(rest),
        "gpt.del"  => {
            if a1.is_empty() || a2.is_empty() { println!("Usage: gpt.del <drive> <partition>"); }
            else { disk_cmds::cmd_gpt_del(a1, a2); }
        }
        "mkswap"   => {
            if a1.is_empty() || a2.is_empty() { println!("Usage: mkswap <drive> <partition>"); }
            else { disk_cmds::cmd_mkswap(a1, a2); }
        }
        "swapon"   => {
            if a1.is_empty() || a2.is_empty() { println!("Usage: swapon <drive> <partition>"); }
            else { disk_cmds::cmd_swapon(a1, a2); }
        }
        "swapoff"    => disk_cmds::cmd_swapoff(),
        "swapinfo"   => disk_cmds::cmd_swapinfo(),
        "blkstat"    => disk_cmds::cmd_blkstat(),
        "blkdiscard" => {
            if a1.is_empty() { println!("Usage: blkdiscard <drive 0-7> [lba count]"); }
            else { disk_cmds::cmd_blkdiscard(rest); }
        }
        "fstrim"     => disk_cmds::cmd_fstrim(),
        "blkonline"  => {
            if a1.is_empty() { println!("Usage: blkonline <drive 0-7>"); }
            else { disk_cmds::cmd_blkonline(a1); }
        }
        "partprobe"  => disk_cmds::cmd_partprobe(rest),
        "nvmestress" => disk_cmds::cmd_nvmestress(),
        "blkzero"    => {
            if a1.is_empty() { println!("Usage: blkzero <drive 0-7> <lba> <count>"); }
            else { disk_cmds::cmd_blkzero(rest); }
        }
        "smart"      => disk_cmds::cmd_smart(a1),
        "swapon.raw" => disk_cmds::cmd_swapon_raw(rest),
        "swapon.auto"=> disk_cmds::cmd_swapon_auto(),
        "mkswap.raw" => disk_cmds::cmd_mkswap_raw(rest),


        "exec" => {
            if a1.is_empty() {
                println!("Usage: exec <path> [args...]");
            } else {
                let argv: alloc::vec::Vec<&str> =
                    core::iter::once(a1)
                    .chain(rest[a1.len()..].split_whitespace())
                    .collect();
                cmd_exec(a1, &argv);
            }
        }

        "history"  => system::cmd_history(),
        "info"     => system::cmd_info(),
        "display"  => display_cmds::cmd_display(rest),
        "memmap"   => system::cmd_memmap(),
        "pstore"   => system::cmd_pstore(rest),
        "dmesg"    => system::cmd_dmesg(rest),
        "help"     => system::cmd_help(),
        "heap"     => system::cmd_heap(),
        // `mm test`, or `run=mm_test` on the GRUB line: the kernel heap and
        // the frame allocator on the machine (`dev/kernel-core.md`)
        "mm"       => match rest.trim() {
            "test" => crate::mm::selftest::cmd_selftest(),
            _ => crate::println!("Usage: mm test"),
        },
        "poweroff" | "shutdown" | "halt" => system::cmd_poweroff(),
        "reboot"   | "restart"           => system::cmd_reboot(),
        "ldconfig" => system::cmd_ldconfig(rest),
        "ldd"      => system::cmd_ldd(a1),
        "nvidia" | "gpu" => nvidia_cmds::cmd_nvidia(rest),
        "mayland" | "may" => mayland_cmds::cmd_mayland(rest),
        "usb" | "lsusb"  => system::cmd_usb(),
        "top"      => system::cmd_top(),
        "swaptest" => system::cmd_swaptest(),
        "nice"     => system::cmd_nice(a1, a2),
        "affinity" => system::cmd_affinity(a1, a2),

        "sv" => cmd_sv(a1, a2, a3),

        // `logout`: ends the console's session (see `cmd_logout`)
        "logout" => cmd_logout(),
        "who" => cmd_who(),

        // `login`: hands the console to a person.
        //
        // The in-kernel shell is ring 0: it does not go through the syscall
        // layer, so no credential check applies. This is a way off it, into a
        // session with an identity, a capability set, and a `mikudo` to ask.
        //
        // `login` holds nothing: it asks mikudod, over the socket the graphical
        // greeter uses. It is started as `greeter`, the account both login
        // prompts run as, with no capability, so a bug in it is an ordinary bug
        "login" => cmd_login(),

        "net"  => { crate::net::poll(); crate::net::cmd_net(rest); }
        "ipc"  => crate::syscall::cmd_ipc(rest),
        "elf"  => elf_cmds::cmd_elf(rest),
        // `ext test`, or `run=ext_test` on the GRUB line. Only what the host
        // test suite cannot reach: two filesystems at once, the real block
        // layer, and the kernel's own mkfs writing to a device
        "ext"  => match rest.trim() {
            "test" => crate::fs::ext::selftest::cmd_selftest(),
            _ => crate::println!("Usage: ext test"),
        },
        // `btrfs test`, or `run=btrfs_test` on the GRUB line. The probe
        // registry, the real block layer, the mount table and a path through
        // `MikuVFS`, which `lib/fs/btrfs`'s host tests cannot reach
        "btrfs" => match rest.trim() {
            "test" => crate::fs::btrfs::selftest::cmd_selftest(),
            _ => crate::println!("Usage: btrfs test"),
        },
        // `vfs test`, or `run=vfs_test` on the GRUB line. The vnode tree driven
        // through its own API against a real filesystem: the layer
        // `lib/fs/extfs`'s host tests sit under
        "vfs"  => match rest.trim() {
            "test" => crate::fs::vfs::selftest::cmd_selftest(),
            _ => crate::println!("Usage: vfs test"),
        },
        // `cred test`, or `run=cred_test` on the GRUB line. Identity, the
        // capability set, and that exec never raises anybody
        "cred" => match rest.trim() {
            "test" => crate::process::selftest_cred::cmd_selftest(),
            "" | "show" => cmd_cred_show(),
            _ => crate::println!("Usage: cred [show|test]"),
        },
        "tty" => match rest.trim() {
            "test" => crate::io::tty::selftest::cmd_selftest(),
            "" | "list" => cmd_tty_list(),
            _ => crate::println!("Usage: tty [list|test]"),
        },
        "dhcp" => crate::net::cmd_dhcp(),
        // ping, traceroute, wget, curl, fetch, ntp and host are programs,
        // reached through the `/bin` fall-through below

        _ => return run_external(cmd, rest),
    }
    true
}

/// Anything the kernel shell does not know is looked for in `/bin`
///
/// So a command can leave ring 0 without the name changing.
///
/// A name with a `/` in it is a path as written; anything else is a program
/// name, and programs live in `/bin`
fn run_external(cmd: &str, rest: &str) -> bool {
    use alloc::string::String;

    let owned: String = if cmd.contains('/') {
        String::from(cmd)
    } else {
        let mut p = String::from("/bin/");
        p.push_str(cmd);
        p
    };

    // Ask first, so the answer can name the reason. The root filesystem is
    // normally the GRUB module and always there; when it is not, "unknown
    // command" would suggest a typo
    if crate::fs::vfs::core::with_vfs(|v| v.resolve_path(0, &owned).is_err()) {
        // Said by the console, not by a ring-3 shell asking on somebody's
        // behalf: that caller has a better message, and a status to return
        if crate::syscall::adapters::admin::in_kcmd() {
            return false;
        }
        crate::print_error!("  unknown command: '{}'", cmd);
        if !cmd.contains('/') {
            crate::println!("  (programs live in /bin - if it is empty, no root filesystem is mounted)");
        }
        return false;
    }

    let mut argv: alloc::vec::Vec<&str> = alloc::vec::Vec::new();
    argv.push(cmd);
    argv.extend(rest.split_whitespace());
    // Interrupts on: a program in ring 3 needs the timer to be scheduled at all
    x86_64::instructions::interrupts::enable();
    cmd_exec(&owned, &argv);
    true
}

fn parse_ip(s: &str) -> Option<[u8; 4]> {
    let mut p = s.split('.');
    Some([p.next()?.parse().ok()?, p.next()?.parse().ok()?,
          p.next()?.parse().ok()?, p.next()?.parse().ok()?])
}

fn cmd_sv_socket(args: &str) {
    let (subcmd, rest) = args.split_once(' ').unwrap_or((args, ""));
    let rest = rest.trim();

    match subcmd {
        "list" | "ls" | "" => {
            let sockets = crate::mikud::list_sockets();
            if sockets.is_empty() {
                crate::println!("  no sockets registered");
                return;
            }
            crate::cprintln!(100, 220, 150, "  {:<14} {:<14} {:<8} {:<8} {}",
                "NAME", "SERVICE", "TYPE", "PORT", "CONNS");
            for s in &sockets {
                crate::println!("  {:<14} {:<14} {:<8} {:<8} {}",
                    s.name, s.service, s.socket_type, s.port, s.connections);
            }
        }
        "stop" => {
            if rest.is_empty() { crate::println!("Usage: sv socket stop <name>"); return; }
            if crate::mikud::stop_socket(rest) {
                crate::cprintln!(100, 220, 150, "  socket '{}' stopped", rest);
            } else {
                crate::print_error!("  socket '{}' not found", rest);
            }
        }
        "remove" | "rm" => {
            if rest.is_empty() { crate::println!("Usage: sv socket remove <name>"); return; }
            if crate::mikud::remove_socket(rest) {
                crate::cprintln!(100, 220, 150, "  socket '{}' removed", rest);
            } else {
                crate::print_error!("  socket '{}' not found", rest);
            }
        }
        _ => {
            crate::println!("Usage: sv socket <list|stop|remove> [name]");
        }
    }
}

fn cmd_sv_timer(args: &str) {
    let (subcmd, rest) = args.split_once(' ').unwrap_or((args, ""));
    let rest = rest.trim();

    match subcmd {
        "list" | "ls" | "" => {
            let timers = crate::mikud::list_timers();
            if timers.is_empty() {
                crate::println!("  no timers registered");
                return;
            }
            crate::cprintln!(100, 220, 150, "  {:<14} {:<14} {:<10} {:<10} {}",
                "NAME", "SERVICE", "TYPE", "INTERVAL", "FIRES");
            for t in &timers {
                crate::println!("  {:<14} {:<14} {:<10} {:<10} {}",
                    t.name, t.service, t.timer_type, t.interval_ticks, t.fire_count);
            }
        }
        "stop" => {
            if rest.is_empty() { crate::println!("Usage: sv timer stop <name>"); return; }
            if crate::mikud::stop_timer(rest) {
                crate::cprintln!(100, 220, 150, "  timer '{}' stopped", rest);
            } else {
                crate::print_error!("  timer '{}' not found", rest);
            }
        }
        "start" => {
            if rest.is_empty() { crate::println!("Usage: sv timer start <name>"); return; }
            if crate::mikud::start_timer(rest) {
                crate::cprintln!(100, 220, 150, "  timer '{}' started", rest);
            } else {
                crate::print_error!("  timer '{}' not found", rest);
            }
        }
        "remove" | "rm" => {
            if rest.is_empty() { crate::println!("Usage: sv timer remove <name>"); return; }
            if crate::mikud::remove_timer(rest) {
                crate::cprintln!(100, 220, 150, "  timer '{}' removed", rest);
            } else {
                crate::print_error!("  timer '{}' not found", rest);
            }
        }
        _ => {
            crate::println!("Usage: sv timer <list|start|stop|remove> [name]");
        }
    }
}

fn cmd_sv(subcmd: &str, name: &str, extra: &str) {
    match subcmd {
        "list" | "ls" | "" => {
            let services = crate::mikud::list_services();
            if services.is_empty() {
                crate::println!("  no services registered");
                return;
            }
            crate::cprintln!(100, 220, 150, "  {:<12} {:<10} {:<8} {}", "NAME", "STATE", "PID", "RESTARTS");
            for svc in &services {
                let pid_str = if svc.pid != 0 {
                    alloc::format!("{}", svc.pid)
                } else {
                    alloc::string::String::from("-")
                };
                crate::println!("  {:<12} {:<10} {:<8} {}", svc.name, svc.state, pid_str, svc.restarts);
            }
        }
        "start" => {
            if name.is_empty() { crate::println!("Usage: sv start <name>"); return; }
            if crate::mikud::start_service(name) {
                crate::cprintln!(100, 220, 150, "  started '{}'", name);
            } else {
                crate::print_error!("  service '{}' not found", name);
            }
        }
        "stop" => {
            if name.is_empty() { crate::println!("Usage: sv stop <name>"); return; }
            if crate::mikud::stop_service(name) {
                crate::cprintln!(100, 220, 150, "  stopped '{}'", name);
            } else {
                crate::print_error!("  service '{}' not found", name);
            }
        }
        "restart" => {
            if name.is_empty() { crate::println!("Usage: sv restart <name>"); return; }
            if crate::mikud::restart_service(name) {
                crate::cprintln!(100, 220, 150, "  restarted '{}'", name);
            } else {
                crate::print_error!("  service '{}' not found", name);
            }
        }
        "status" => {
            if name.is_empty() { crate::println!("Usage: sv status <name>"); return; }
            let services = crate::mikud::list_services();
            if let Some(svc) = services.iter().find(|svc| svc.name == name) {
                // State line with color
                let state_color = match svc.state {
                    "running" | "activating" => (100, 220, 150),
                    "failed" => (255, 80, 80),
                    "stopped" | "dead" => (160, 160, 160),
                    "starting" | "reloading" | "stopping" => (220, 200, 80),
                    _ => (200, 200, 200),
                };
                crate::cprintln!(state_color.0, state_color.1, state_color.2,
                    "  {} - {}", svc.name, svc.state);
                if !svc.description.is_empty() {
                    crate::println!("    Description: {}", svc.description);
                }
                crate::println!("    Type:        {}", svc.svc_type);
                crate::println!("    Target:      {}", svc.target);
                let pid_str = if svc.pid != 0 {
                    alloc::format!("{}", svc.pid)
                } else {
                    alloc::string::String::from("-")
                };
                crate::println!("    PID:         {}", pid_str);
                if let Some(path) = svc.exec_start_path {
                    crate::println!("    ExecStart:   {}", path);
                }
                crate::println!("    Restart:     {} (count={})", svc.restart, svc.restarts);
                crate::println!("    Exit code:   {}", svc.last_exit_code);
                if !svc.deps.is_empty() {
                    crate::print!("    Requires:    ");
                    for d in svc.deps { crate::print!("{} ", d); }
                    crate::println!();
                }
                if !svc.wants.is_empty() {
                    crate::print!("    Wants:       ");
                    for w in svc.wants { crate::print!("{} ", w); }
                    crate::println!();
                }
                if !svc.conflicts.is_empty() {
                    crate::print!("    Conflicts:   ");
                    for c in svc.conflicts { crate::print!("{} ", c); }
                    crate::println!();
                }
                if svc.watchdog_ticks > 0 {
                    crate::println!("    Watchdog:    {} ticks", svc.watchdog_ticks);
                }
                if svc.critical {
                    crate::cprintln!(255, 200, 80, "    Flags:       critical");
                }
                if svc.masked {
                    crate::cprintln!(160, 160, 160, "    Flags:       masked");
                }
                // Recent journal entries for this service
                let entries = crate::mikud::journal::entries_for_service(name);
                if !entries.is_empty() {
                    crate::println!("    Journal:");
                    let start = if entries.len() > 5 { entries.len() - 5 } else { 0 };
                    for e in &entries[start..] {
                        crate::println!("      t={} {} pid={} code={}",
                            e.tick, e.event.as_str(), e.pid, e.code);
                    }
                }
            } else {
                crate::print_error!("  service '{}' not found", name);
            }
        }
        "journal" | "log" => {
            let entries = if name.is_empty() {
                crate::mikud::journal::recent(20)
            } else {
                crate::mikud::journal::entries_for_service(name)
            };
            if entries.is_empty() {
                crate::println!("  no journal entries");
                return;
            }
            crate::cprintln!(100, 220, 150, "  {:<8} {:<3} {:<12} {:<8} {}", "TICK", "E", "SERVICE", "PID", "CODE");
            for e in &entries {
                crate::println!("  {:<8} {:<3} {:<12} {:<8} {}",
                    e.tick, e.event.symbol(), e.service, e.pid, e.code);
            }
            crate::println!("  ({} total events)", crate::mikud::journal::total_events());
        }
        "enable" => {
            if name.is_empty() { crate::println!("Usage: sv enable <name>"); return; }
            if crate::mikud::enable_service(name) {
                crate::cprintln!(100, 220, 150, "  enabled '{}'", name);
            } else {
                crate::print_error!("  service '{}' not found", name);
            }
        }
        "disable" => {
            if name.is_empty() { crate::println!("Usage: sv disable <name>"); return; }
            if crate::mikud::disable_service(name) {
                crate::cprintln!(100, 220, 150, "  disabled '{}'", name);
            } else {
                crate::print_error!("  service '{}' not found", name);
            }
        }
        "reload" => {
            if name.is_empty() { crate::println!("Usage: sv reload <name>"); return; }
            if crate::mikud::reload_service(name) {
                crate::cprintln!(100, 220, 150, "  reloaded '{}' (SIGHUP)", name);
            } else {
                crate::print_error!("  failed to reload '{}'", name);
            }
        }
        "mask" => {
            if name.is_empty() { crate::println!("Usage: sv mask <name>"); return; }
            if crate::mikud::mask_service(name) {
                crate::cprintln!(100, 220, 150, "  masked '{}'", name);
            } else {
                crate::print_error!("  failed to mask '{}'", name);
            }
        }
        "unmask" => {
            if name.is_empty() { crate::println!("Usage: sv unmask <name>"); return; }
            if crate::mikud::unmask_service(name) {
                crate::cprintln!(100, 220, 150, "  unmasked '{}'", name);
            } else {
                crate::print_error!("  failed to unmask '{}'", name);
            }
        }
        "force-stop" | "kill" => {
            if name.is_empty() { crate::println!("Usage: sv force-stop <name>"); return; }
            if crate::mikud::force_stop_service(name) {
                crate::cprintln!(100, 220, 150, "  force-stopped '{}'", name);
            } else {
                crate::print_error!("  failed to force-stop '{}'", name);
            }
        }
        "target" => {
            if name.is_empty() {
                // Show current and default target
                crate::println!("  current: {}", crate::mikud::target_name());
                crate::println!("  default: {}", crate::mikud::default_target().as_str());
                let (phase, done) = crate::mikud::boot_state();
                crate::println!("  phase:   {} (boot {})", phase, if done { "complete" } else { "in progress" });
                crate::println!("  services: {}/{} active",
                    crate::mikud::active_service_count(), crate::mikud::service_count());
            } else if name == "isolate" {
                crate::println!("Usage: sv target isolate <sysinit|multi-user|graphical|rescue>");
            } else {
                // Try to set target
                if crate::mikud::set_target_name(name) {
                    crate::cprintln!(100, 220, 150, "  target -> {}", name);
                } else {
                    crate::print_error!("  unknown target '{}' (sysinit|multi-user|graphical|rescue)", name);
                }
            }
        }
        "analyze" => {
            let timings = crate::mikud::boot_analyze();
            if timings.is_empty() {
                crate::println!("  no boot data");
                return;
            }
            crate::cprintln!(100, 220, 150, "  {:<14} {:<12} {:<10} {}",
                "SERVICE", "TARGET", "START", "DURATION");
            for t in &timings {
                let dur = if t.duration_ticks == 0 {
                    alloc::string::String::from("instant")
                } else {
                    alloc::format!("{} ticks", t.duration_ticks)
                };
                crate::println!("  {:<14} {:<12} {:<10} {}", t.name, t.target, t.start_tick, dur);
            }
        }
        "tree" => {
            if name.is_empty() { crate::println!("Usage: sv tree <name>"); return; }
            let tree = crate::mikud::dependency_tree(name);
            if tree.is_empty() {
                crate::print_error!("  service '{}' not found", name);
                return;
            }
            for (dep_name, depth) in &tree {
                let indent = "  ".repeat(*depth as usize + 1);
                let marker = if *depth == 0 { "*" } else { "-" };
                let state = crate::mikud::service_state(dep_name)
                    .map(|s| s.as_str())
                    .unwrap_or("?");
                crate::println!("{}{} {} ({})", indent, marker, dep_name, state);
            }
        }
        "rdeps" => {
            if name.is_empty() { crate::println!("Usage: sv rdeps <name>"); return; }
            let deps = crate::mikud::reverse_deps(name);
            if deps.is_empty() {
                crate::println!("  no services depend on '{}'", name);
            } else {
                crate::println!("  services that depend on '{}':", name);
                for d in &deps {
                    let state = crate::mikud::service_state(d)
                        .map(|s| s.as_str())
                        .unwrap_or("?");
                    crate::println!("    {} ({})", d, state);
                }
            }
        }
        "load" => {
            if name.is_empty() { crate::println!("Usage: sv load <path.service>"); return; }
            crate::mikud::unit::load_unit_file(name);
        }
        "scan" => {
            crate::mikud::unit::scan_unit_dir();
        }
        "isolate" => {
            if name.is_empty() {
                crate::println!("Usage: sv isolate <sysinit|multi-user|graphical|rescue>");
                return;
            }
            match crate::mikud::Target::from_str(name) {
                Some(target) => {
                    crate::mikud::isolate_target(target);
                    crate::cprintln!(100, 220, 150, "  isolated to target '{}'", name);
                }
                None => {
                    crate::print_error!("  unknown target '{}' (sysinit|multi-user|graphical|rescue)", name);
                }
            }
        }
        "timer" => {
            let timer_args = if extra.is_empty() {
                alloc::string::String::from(name)
            } else {
                alloc::format!("{} {}", name, extra)
            };
            cmd_sv_timer(&timer_args);
        }
        "socket" | "sock" => {
            let sock_args = if extra.is_empty() {
                alloc::string::String::from(name)
            } else {
                alloc::format!("{} {}", name, extra)
            };
            cmd_sv_socket(&sock_args);
        }
        "cat" => {
            if name.is_empty() { crate::println!("Usage: sv cat <name>"); return; }
            // Show unit file summary from registered service
            let services = crate::mikud::list_services();
            if let Some(svc) = services.iter().find(|s| s.name == name) {
                crate::cprintln!(100, 220, 150, "  [Unit]");
                if !svc.description.is_empty() {
                    crate::println!("  Description={}", svc.description);
                }
                if !svc.deps.is_empty() {
                    crate::print!("  Requires=");
                    for d in svc.deps { crate::print!("{} ", d); }
                    crate::println!();
                }
                if !svc.wants.is_empty() {
                    crate::print!("  Wants=");
                    for w in svc.wants { crate::print!("{} ", w); }
                    crate::println!();
                }
                if !svc.conflicts.is_empty() {
                    crate::print!("  Conflicts=");
                    for c in svc.conflicts { crate::print!("{} ", c); }
                    crate::println!();
                }
                crate::cprintln!(100, 220, 150, "  [Service]");
                crate::println!("  Type={}", svc.svc_type);
                if let Some(path) = svc.exec_start_path {
                    crate::println!("  ExecStart={}", path);
                }
                crate::println!("  Restart={}", svc.restart);
                if svc.watchdog_ticks > 0 {
                    crate::println!("  WatchdogSec={}", crate::mikud::unit::ticks_as_duration(svc.watchdog_ticks));
                }
                if svc.critical {
                    crate::println!("  Critical=true");
                }
                if svc.masked {
                    crate::println!("  # MASKED");
                }
                crate::cprintln!(100, 220, 150, "  [Install]");
                crate::println!("  WantedBy={}", svc.target);
            } else {
                crate::print_error!("  service '{}' not found", name);
            }
        }
        // A service talking about itself: `READY=1` (a `Type=notify` service
        // has finished starting) and `WATCHDOG=1` (it is still alive), as in
        // sd_notify. About the caller's own service and no other, so it is not
        // privileged (`gated_by`)
        "notify" => {
            let mut ready = false;
            let mut watchdog = false;
            for word in [name, extra] {
                match word {
                    "" => {}
                    "READY=1" => ready = true,
                    "WATCHDOG=1" => watchdog = true,
                    other => {
                        crate::print_error!("  sv notify: {}: not READY=1 or WATCHDOG=1", other);
                        return;
                    }
                }
            }
            if !ready && !watchdog {
                crate::println!("Usage: sv notify [READY=1] [WATCHDOG=1]");
                return;
            }
            match crate::mikud::notify_from(crate::sched::current_pid(), ready, watchdog) {
                Some(svc) => crate::println!("  {}: noted", svc),
                None => crate::print_error!("  sv notify: this process is not part of any service"),
            }
        }
        _ => {
            crate::println!("Usage: sv <command> [name]");
            crate::println!("  list          - list all services");
            crate::println!("  status <name> - detailed service status");
            crate::println!("  cat <name>    - show service unit config");
            crate::println!("  start <name>  - start a service");
            crate::println!("  stop <name>   - stop a service");
            crate::println!("  restart <name> - restart a service");
            crate::println!("  reload <name> - send SIGHUP to service");
            crate::println!("  enable/disable <name> - enable/disable service");
            crate::println!("  mask/unmask <name> - prevent/allow service start");
            crate::println!("  force-stop <name> - force kill (even critical)");
            crate::println!("  journal [name] - show event log");
            crate::println!("  target [name] - show/set active target");
            crate::println!("  isolate <tgt> - switch target, stop unneeded");
            crate::println!("  analyze       - boot timing analysis");
            crate::println!("  tree <name>   - dependency tree");
            crate::println!("  rdeps <name>  - reverse dependencies");
            crate::println!("  load <path>   - load .service unit file");
            crate::println!("  scan          - scan /etc/mikud/ for units");
            crate::println!("  timer         - manage timer units");
            crate::println!("  socket        - manage socket units");
            crate::println!("  notify READY=1|WATCHDOG=1 - a service reports on itself");
        }
    }
}


/// `cred`: who this shell is, and what it may do
///
/// After `mikudo` or a service has changed something, this says what the kernel
/// believes rather than what /etc/passwd claims
fn cmd_cred_show() {
    let cred = crate::sched::current_cred();
    crate::println!("uid={} euid={} suid={}", cred.uid, cred.euid, cred.suid);
    crate::println!("gid={} egid={} sgid={}", cred.gid, cred.egid, cred.sgid);
    if cred.ngroups > 0 {
        crate::print!("groups=");
        for i in 0..cred.ngroups as usize {
            if i > 0 { crate::print!(","); }
            crate::print!("{}", cred.groups[i]);
        }
        crate::println!("");
    }
    crate::println!("umask={:04o}", cred.umask);
    crate::println!("caps  effective={:#06x} permitted={:#06x} bounding={:#06x}",
        cred.effective.0, cred.permitted.0, cred.bounding.0);
    if cred.effective == crate::process::cred::CAP_ALL {
        crate::println!("      (every capability - this is what 'root' means here)");
    } else {
        crate::print!("      ");
        for i in 0..=crate::process::cred::CAP_LAST {
            if cred.effective.has(i) {
                crate::print!("{} ", crate::process::cred::CapSet::name(i));
            }
        }
        crate::println!("");
    }
}


/// Which capability a command wants, or `None` if anybody may run it
///
/// `admin_only` answers whether a command is the machine's; this answers which
/// authority it takes.
///
/// Reading is not administering: `nvidia-smi` does not want root on Linux and
/// neither does `lsusb`, because looking at what the hardware says about itself
/// is not a privileged act; writing a partition table is. The split is by what
/// the command does, so the first argument matters: `nvidia list` and
/// `nvidia gsp-rm-boot` differ.
///
/// The conservative half of the line is where Linux draws it. Register dumps,
/// scans, pokes and boot sequences are privileged even though they only read: a
/// register read is not always free of consequence. Raw filesystem structures
/// (`ext*`, `gpt`, `blk` writes) stay privileged as `/dev/sda` is not
/// world-readable
fn gated_by(cmd: &str, rest: &str) -> Option<u8> {
    use crate::process::cred::{CAP_SYS_ADMIN, CAP_SYS_BOOT};
    match cmd {
        "reboot" | "restart" | "poweroff" | "shutdown" | "halt" => Some(CAP_SYS_BOOT),
        // Not read-only, and not privileged either: it changes only the state
        // of the service the caller is itself part of (`mikud::notify_from`),
        // which any service has to be able to do as whatever user it runs as
        "sv" if rest.split_whitespace().next() == Some("notify") => None,
        _ if read_only(cmd, rest) => None,
        _ if admin_only(cmd) => Some(CAP_SYS_ADMIN),
        _ => None,
    }
}

/// Does this command only look at things?
///
/// The subsystem commands take a verb, and the verb decides. Anything not named
/// here is assumed to change something
fn read_only(cmd: &str, rest: &str) -> bool {
    let verb = rest.split_whitespace().next().unwrap_or("");
    match cmd {
        // What `nvidia-smi` shows without root: which cards are here, what they
        // are doing, how warm they are, what is plugged into them.
        //
        // `nvidia perf` shows; `nvidia perf boost` changes the clocks
        "nvidia" | "gpu" if verb == "perf" => !rest.split_whitespace().any(|w| w == "boost"),
        // The listing, `imp` and `underflow` look; `poll` acts on a hot-plug.
        // The setters (mode, on, off, vrr) are in `display`
        "nvidia" | "gpu" if verb == "disp" || verb == "outputs" =>
            rest.split_whitespace().nth(1) != Some("poll"),
        "nvidia" | "gpu" => matches!(verb,
            "" | "info" | "status" | "list" | "ls" | "gpus"
            | "temp" | "thermal" | "temperature"
            | "vram" | "fbmap" | "va" | "vaspace" | "disp" | "outputs"
            | "util" | "pcie" | "bus"
            | "firmware" | "fw" | "rm-tree" | "objects" | "rmtree"
            | "gb206" | "blackwell" | "5060"
            | "next" | "todo" | "roadmap" | "help" | "?"),

        // `systemctl status` is not privileged either
        "sv" => matches!(verb,
            "" | "list" | "ls" | "status" | "tree" | "rdeps" | "analyze"
            | "failed" | "cat" | "show" | "help"),

        // `mount` with nothing to mount is `mount`, which prints the table
        "mount" => verb.is_empty(),

        // Which devices exist and how big they are: `lsblk`, not `fdisk`
        "blkstat" | "swapinfo" | "usb" | "lsusb" => true,

        // What the screens are: `xrandr` with no arguments. Changing one is not
        // here
        "display" => matches!(verb, "" | "list" | "ls" | "help" | "?"),

        // The compositor's own state, for a program that wants to know whether
        // there is a desktop. Starting and stopping it is not here
        "mayland" | "may" => matches!(verb, "" | "status" | "info" | "list" | "help"),

        _ => false,
    }
}

/// The commands that are the machine's rather than a user's.
///
/// Most reach a disk directly: the `ext*` family walks inodes on a mounted
/// filesystem with no reference to who is asking, and `dd` reads a block
/// device, so the permission bits do not apply and `ext2cat` would print
/// /etc/shadow. The rest are machine operations (partitioning, swap, firmware,
/// service control).
///
/// Teaching each of them the credential model is the long-term answer; refusing
/// them fails loudly rather than leaking
fn admin_only(cmd: &str) -> bool {
    // Anything that walks a filesystem's raw structures, plus dd
    if cmd.starts_with("ext") || cmd.starts_with("mkfs.") || cmd.starts_with("fs.")
        || cmd.starts_with("gpt") || cmd.starts_with("blk") || cmd.starts_with("swap")
        || cmd.starts_with("mkswap")
    {
        return true;
    }
    matches!(cmd,
        // raw block and filesystem access
        "dd" | "mount" | "umount" | "fstrim" | "partprobe" | "smart" | "nvmestress"
        | "fiemap" | "chattr" | "lsattr" | "getxattr" | "setxattr" | "listxattr"
        | "install" | "scan"
        // the machine itself
        | "reboot" | "poweroff" | "nvidia" | "display" | "usb" | "memmap" | "pstore" | "load"
        // The kernel's log: addresses, and what every other user's programs did
        | "dmesg"
        // Service control: starting and stopping the machine's services,
        // `mikudod` among them
        | "start" | "stop" | "restart" | "reload" | "enable" | "disable"
        | "mask" | "unmask" | "isolate" | "target" | "force-stop" | "sv"
    )
}

/// Ends the console's session.
///
/// The console is physical access only on a text boot; on a graphical one it is
/// a window (`/bin/mayterm`), and the person typing is inside their own login.
/// So this ends the session and stops there: `logout` must never yield root.
/// Authority comes back through `login`, a program asking a broker, and on a
/// graphical boot through `shell::adopt_terminal_owner`, which reads whose
/// terminal this is
fn cmd_logout() {
    let was = crate::sched::current_cred();
    if was.uid == 0 {
        crate::println!("  logout: nobody is logged in - the console is already root");
        return;
    }
    // A console session is the machine's own login, and closing it is what
    // puts the credential back and hands the seat on
    let closed = {
        let mut buf = [Default::default(); crate::process::session::MAX_SESSIONS];
        let n = crate::process::session::list_console(&mut buf);
        let mut done = false;
        for s in &buf[..n] {
            if crate::process::session::close(s.id) { done = true; }
        }
        done
    };
    if closed {
        crate::serial_println!("[session] uid {} logged out; the console is root again", was.uid);
        crate::print_success!("  logged out - the console is root again. 'login' to start a session");
        return;
    }

    // No console session to close: the graphical case. The terminal belongs to
    // a graphical session, which the desktop's log-out ends.
    //
    // Same person, no capability: `Cred::user` is an ordinary account, the only
    // direction this command may move
    crate::sched::update_current_cred(|c| *c = crate::process::cred::Cred::user(was.uid, was.gid));
    crate::serial_println!(
        "[session] uid {} asked to log out of a console with no session - \
         authority dropped, not raised", was.uid);
    crate::print_success!(
        "  this terminal has no console session to end - it belongs to your \
         desktop session. Authority dropped; use 'login' to start one");
}

/// Starts the console's login prompt as the account it belongs to
///
/// `greeter` is a member of `session`, the group mikudod checks before opening
/// a session. It has no password, no capabilities and nothing to read.
///
/// The identity is applied here, after `exec` has built the image and before
/// the scheduler has run an instruction of it, by the function mikuD uses for a
/// unit's `User=`
pub fn run_login() {
    cmd_login()
}

fn cmd_login() {
    const PATH: &str = "/bin/login";
    const AS_WHOM: &str = "greeter";

    let who = crate::syscall::adapters::process::passwd_ids(AS_WHOM);
    if who.is_none() {
        crate::print_error!("login: this machine has no '{}' account", AS_WHOM);
        crate::println!("  (it is in the shipped /etc/passwd; a disk without one has no login)");
        return;
    }

    match crate::process::exec::exec(PATH, &[PATH], crate::process::exec::DEFAULT_ENV) {
        Ok(pid) => {
            // No capabilities, whoever it inherited them from. `Some(0)` is
            // "these and no others", unlike `None`
            crate::mikud::apply_unit_identity(pid, who, Some(0), "login");
            // login reads the keyboard, so it needs the console, and it is
            // waited for so the prompt underneath is not drawn over the
            // password prompt
            crate::io::input::user_stdin::set_foreground(pid);
            crate::sched::waitpid(pid);
            crate::io::input::user_stdin::clear_foreground();
        }
        Err(e) => crate::print_error!("login: {}", e.as_str()),
    }
}

/// `who`: the sessions open on this machine
///
/// Reads the table directly rather than through syscall 92: this is ring-0 code
fn cmd_who() {
    let mut buf = [Default::default(); crate::process::session::MAX_SESSIONS];
    let n = crate::process::session::list(&mut buf);
    if n == 0 {
        crate::println!("  nobody is logged in");
        return;
    }
    let mut namebuf = [0u8; 33];
    for s in &buf[..n] {
        let name = crate::syscall::adapters::process::passwd_name(s.uid, &mut namebuf)
            .unwrap_or("?");
        if s.leader == 0 {
            crate::println!("  {:<12} {:<10} seat{}  session {}",
                            name, s.kind.as_str(), s.seat, s.id);
        } else {
            crate::println!("  {:<12} {:<10} seat{}  session {}  leader pid {}",
                            name, s.kind.as_str(), s.seat, s.id, s.leader);
        }
    }
}

/// `tty`: what terminals this machine has
fn cmd_tty_list() {
    for id in crate::io::tty::list() {
        let who = match crate::io::tty::master_owner(id) {
            Some(pid) => alloc::format!("pid {}", pid),
            None if id == crate::io::tty::SYSTEM => alloc::string::String::from("the console"),
            None => alloc::string::String::from("hung up"),
        };
        let shape = crate::io::tty::with_grid(id, |g| g.size()).unwrap_or((0, 0));
        crate::println!("  tty{}  {:>3}x{:<3}  {}", id.index(), shape.0, shape.1, who);
    }
}
