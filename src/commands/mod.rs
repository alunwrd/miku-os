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

fn cmd_exec(path: &str, args: &[&str]) {
    match crate::exec_elf::exec(path, args) {
        Ok(pid) => {
            crate::scheduler::waitpid(pid);
            crate::user_stdin::clear_foreground();
        }
        Err(e) => {
            crate::print_error!("  exec: {}", e.as_str());
        }
    }
}

pub fn execute(input: &str) {
    let t = input.trim();
    if t.is_empty() { return; }

    let mut parts = t.split_whitespace();
    let cmd  = parts.next().unwrap_or("");
    let a1   = parts.next().unwrap_or("");
    let a2   = parts.next().unwrap_or("");
    let a3   = parts.next().unwrap_or("");
    let rest = if t.len() > cmd.len() { t[cmd.len()..].trim_start() } else { "" };

    match cmd {
        "ls"  => fs::cmd_ls(if a1.is_empty() { "." } else { a1 }),
        "cd"  => fs::cmd_cd(a1),
        "pwd" => fs::cmd_pwd(),

        "mkdir" => {
            if a1.is_empty() { println!("Usage: mkdir <name>"); }
            else { fs::cmd_mkdir(a1); }
        }
        "touch" => {
            if a1.is_empty() { println!("Usage: touch <name>"); }
            else { fs::cmd_touch(a1); }
        }
        "cat" => {
            if a1.is_empty() { println!("Usage: cat <file>"); }
            else { fs::cmd_cat(a1); }
        }
        "write" => {
            if a1.is_empty() || rest.len() <= a1.len() { println!("Usage: write <file> <text>"); }
            else { fs::cmd_write(a1, rest[a1.len()..].trim_start()); }
        }
        "stat" => {
            if a1.is_empty() { println!("Usage: stat <path>"); }
            else { fs::cmd_stat(a1); }
        }
        "rm" => {
            if a1.is_empty() { println!("Usage: rm [-rf] <path>"); }
            else if a1 == "-rf" || a1 == "-r" || a1 == "-f" {
                if a2.is_empty() { println!("Usage: rm -rf <path>"); }
                else { fs::cmd_rm_rf(a2); }
            } else {
                fs::cmd_rm(a1);
            }
        }
        "rmdir" => {
            if a1.is_empty() { println!("Usage: rmdir <dir>"); }
            else { fs::cmd_rmdir(a1); }
        }
        "mv" => {
            if a1.is_empty() || a2.is_empty() { println!("Usage: mv <old> <new>"); }
            else { fs::cmd_mv(a1, a2); }
        }
        "ln" => {
            if a1 == "-s" {
                if a2.is_empty() || a3.is_empty() { println!("Usage: ln -s <target> <linkname>"); }
                else { fs::cmd_symlink(a2, a3); }
            } else {
                if a1.is_empty() || a2.is_empty() { println!("Usage: ln <existing> <newname>"); }
                else { fs::cmd_link(a1, a2); }
            }
        }
        "readlink" => {
            if a1.is_empty() { println!("Usage: readlink <path>"); }
            else { fs::cmd_readlink(a1); }
        }
        "chmod" => {
            if a1.is_empty() || a2.is_empty() { println!("Usage: chmod <mode> <path>"); }
            else { fs::cmd_chmod(a1, a2); }
        }
        "df"     => fs::cmd_df(),
        "mount"  => { if a1.is_empty() { fs::cmd_mount_list(); } else { fs::cmd_mount(a1, a2); } }
        "umount" => { if a1.is_empty() { println!("Usage: umount <path>"); } else { fs::cmd_umount(a1); } }

        "ext2mount" => ext2_cmds::cmd_ext2_mount(rest),
        "ext3mount" => ext3_cmds::cmd_ext3_mount(rest),
        "ext4mount" => ext4_cmds::cmd_ext4_mount(rest),

        "fs.list"   => ext2_cmds::cmd_fs_list(),
        "fs.select" => ext2_cmds::cmd_fs_select(rest),
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
            else { fs::cmd_rmdir(a1); }
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
        "extfsck" => { ext_dispatch!(
            ext2_cmds::cmd_ext2_fsck(),
            ext2_cmds::cmd_ext2_fsck(),
            ext4_cmds::cmd_ext4_fsck()
        ); }
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
        "ext2fsck"      => ext2_cmds::cmd_ext2_fsck(),
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
        "ext4fsck"   => ext4_cmds::cmd_ext4_fsck(),

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

        "echo"     => system::cmd_echo(rest),
        "history"  => system::cmd_history(),
        "info"     => system::cmd_info(),
        "memmap"   => system::cmd_memmap(),
        "help"     => system::cmd_help(),
        "clear"    => system::cmd_clear(),
        "heap"     => system::cmd_heap(),
        "poweroff" | "shutdown" | "halt" => system::cmd_poweroff(),
        "reboot"   | "restart"           => system::cmd_reboot(),
        "ps"       => system::cmd_ps(),
        "ldconfig" => system::cmd_ldconfig(rest),
        "ldd"      => system::cmd_ldd(a1),
        "nvidia" | "gpu" => nvidia_cmds::cmd_nvidia(rest),
        "top"      => system::cmd_top(),
        "swaptest" => system::cmd_swaptest(),
        "nice"     => system::cmd_nice(a1, a2),
        "affinity" => system::cmd_affinity(a1, a2),
        "kill"     => {
            if a1.is_empty() { println!("Usage: kill <pid>"); }
            else if let Ok(pid) = a1.parse::<u64>() {
                crate::scheduler::kill(pid);
                crate::cprintln!(100, 220, 150, "  killed pid={}", pid);
            } else {
                crate::print_error!("  invalid pid");
            }
        }

        "sv" => cmd_sv(a1, a2, a3),

        "net"  => { crate::net::poll(); crate::net::cmd_net(rest); }
        "dhcp" => crate::net::cmd_dhcp(),
        "ping" => {
            if a1.is_empty() { println!("Usage: ping <ip|host> [count]"); }
            else {
                let count = a2.parse::<usize>().unwrap_or(usize::MAX);
                match parse_ip(a1) {
                    Some(ip) => crate::net::cmd_ping(a1, &ip, count),
                    None => {
                        crate::cprintln!(57, 197, 187, "ping: resolving {}...", a1);
                        let dns = crate::net::get_dns();
                        match crate::net::dns::resolve(a1, &dns) {
                            Some(ip) => crate::net::cmd_ping(a1, &ip, count),
                            None => crate::print_error!("ping: cannot resolve '{}'", a1),
                        }
                    }
                }
            }
        }
        "fetch" => {
            if a1.is_empty() { println!("Usage: fetch <url|host> [port]"); }
            else { cmd_fetch(a1, a2); }
        }
        "wget"  => {
            if a1.is_empty() { println!("Usage: wget <url> [-O <file>]"); }
            else {
                x86_64::instructions::interrupts::enable();
                crate::net::http::cmd_wget(rest);
            }
        }
        "curl"  => {
            if a1.is_empty() { println!("Usage: curl <url> [-X GET|POST] [-d <data>] [-o <file>] [-I]"); }
            else {
                x86_64::instructions::interrupts::enable();
                crate::net::http::cmd_curl(rest);
            }
        }
        "ntp"   => {
            x86_64::instructions::interrupts::enable();
            crate::net::ntp::cmd_ntp(a1);
        }
        "traceroute" | "tr" => {
            if a1.is_empty() { println!("Usage: traceroute <host|ip>"); }
            else {
                x86_64::instructions::interrupts::enable();
                crate::net::traceroute::cmd_traceroute(a1);
            }
        }

        _ => println!("Unknown: '{}'", cmd),
    }
}

fn cmd_fetch(host_or_url: &str, port_str: &str) {
    if host_or_url.starts_with("http://") || host_or_url.starts_with("https://") {
        x86_64::instructions::interrupts::enable();
        if let Some(resp) = crate::net::http::get(host_or_url) {
            let sc = if resp.status < 400 { (100u8, 220u8, 150u8) } else { (255u8, 80u8, 80u8) };
            crate::cprintln!(sc.0, sc.1, sc.2, "HTTP {} {}", resp.status, resp.reason);
            if resp.body.is_empty() {
                crate::print_warn!("fetch: empty body");
            } else {
                crate::cprintln!(120, 200, 200, "fetch: {} bytes", resp.body.len());
                print_response(&resp.body);
            }
        }
        return;
    }
    let (host, port, use_tls) = {
        let p: u16 = port_str.parse().unwrap_or(80);
        (host_or_url, p, p == 443)
    };
    let dns = crate::net::get_dns();
    let ip  = match parse_ip(host) {
        Some(ip) => ip,
        None => {
            crate::cprintln!(57, 197, 187, "fetch: resolving {}...", host);
            match crate::net::dns::resolve(host, &dns) {
                Some(ip) => ip,
                None => { crate::print_error!("fetch: cannot resolve '{}'", host); return; }
            }
        }
    };
    crate::cprintln!(57, 197, 187,
        "fetch: connecting to {}.{}.{}.{}:{} ({})...",
        ip[0], ip[1], ip[2], ip[3], port,
        if use_tls { "TLS" } else { "plain" }
    );
    x86_64::instructions::interrupts::enable();
    let req_str  = alloc::format!(
        "GET / HTTP/1.1\r\nHost: {}\r\nUser-Agent: MikuOS/0.2-rc\r\nConnection: close\r\n\r\n",
        host
    );
    let req_bytes = req_str.as_bytes();
    if use_tls {
        crate::cprintln!(120, 200, 200, "fetch: TLS handshake...");
        let mut stream = match crate::net::tls::TlsStream::connect(host, ip, port) {
            Some(s) => s,
            None    => { crate::print_error!("fetch: TLS failed"); return; }
        };
        crate::print_success!("fetch: TLS ok ({})", stream.cipher_name());
        if !stream.send(req_bytes) { crate::print_error!("fetch: send failed"); stream.close(); return; }
        crate::cprintln!(120, 200, 200, "fetch: waiting...");
        let data = stream.recv_all(8_000_000);
        print_response(data);
        stream.close();
    } else {
        let mut sock = match crate::net::tcp::TcpSocket::connect(ip, port) {
            Some(s) => s,
            None    => { crate::print_error!("fetch: connection failed"); return; }
        };
        crate::print_success!("fetch: connected");
        if !sock.send(req_bytes) { crate::print_error!("fetch: send failed"); sock.close(); return; }
        crate::cprintln!(120, 200, 200, "fetch: waiting...");
        let data = sock.recv_all(8_000_000);
        print_response(data);
        sock.close();
    }
}

fn print_response(data: &[u8]) {
    if data.is_empty() { crate::print_warn!("fetch: no data received"); return; }
    let show = data.len().min(4096);
    let mut text = alloc::string::String::with_capacity(show);
    for &b in &data[..show] {
        if b == b'\n' || b == b'\r' || b == b'\t' || (b >= 32 && b <= 126) {
            text.push(b as char);
        } else {
            text.push('.');
        }
    }
    crate::println!("{}", text);
    if data.len() > show {
        crate::cprintln!(120, 140, 140, "... ({} bytes total, showing first 4096)", data.len());
    }
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
                    crate::println!("  WatchdogSec={}", svc.watchdog_ticks);
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
        }
    }
}
