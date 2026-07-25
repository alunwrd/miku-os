<div align="center">

# Miku OS

**Rustで開発された実験的なオペレーティングシステムカーネル**

*Rustと一人の開発者によって動いています :D*

<img src="https://raw.githubusercontent.com/alunwrd/miku-os/main/docs/miku.png" width="220" alt="Miku Logo">

[![Language](https://img.shields.io/badge/language-Rust-orange.svg)](https://www.rust-lang.org/)
[![Architecture](https://img.shields.io/badge/arch-x86__64-blue.svg)]()
[![Status](https://img.shields.io/badge/status-experimental-yellow.svg)]()
[![License](https://img.shields.io/badge/license-MIT-lightgrey.svg)]()

</div>

---

> **翻訳:** [English](English_README.md) · [Russian](Russian_README.md) · [メインREADME](../README.md)
> **ABI仕様:** [MikuOS_ABI.md](MikuOS_ABI.md)
> **GPUメモ:** [TU116](Nvidia_tu116.md) · [GB206](Nvidia_gb206.md)

---

## プロジェクトについて

**Miku OS** はRustの `no_std` 環境でゼロから構築されたオペレーティングシステムです。下層にlibcは
なく、ホストランタイムもありません。メモリレイアウト、割り込み処理、スケジューリング、ファイル
システム、ドライバはすべてここに書かれています。

現在動作しているもの: プリエンプティブなSMPカーネル、`fork`/`exec`/`wait` とシグナルを備えた
ring 3のユーザプロセス、共有ライブラリ対応の動的リンカ、initデーモン (mikuD)、ext2/ext3/ext4を
扱うVFS、ライトバックキャッシュ付きブロック層、TCP/IPスタック、そしてユーザ空間シェル。

> コードはすべてRustです。アセンブリはブートエントリ、syscallブリッジ、APトランポリン、
> コンテキストスイッチにのみ現れます。呼び出し規約が厳密でなければならない箇所です。

**規模:** カーネル251ファイル約71,500行、加えてユーザ空間のライブラリとプログラム約30,700行。

---

## 現状と正直な制限

これは実験的なシステムです。以下は実際に欠けているもの、暫定的なものの一覧です。後述の機能表を
実態以上に読まないための注意書きです。

| 領域 | 状態 |
|:--|:--|
| **プリエンプション** | 新規。QEMUの1-16 CPUで検証済み。以前のnaked-asmタイマエントリは実機をハングさせたため、`kernel/arch/x86_64/interrupts.rs` の `PREEMPTIVE_TIMER` で切り替え可能。実機が最初のティック後にハングする場合は `false` にすると協調的スケジューリングに戻ります |
| **ユーザ空間スレッド** | なし。`clone` も `futex` もありません。複数CPUを使うのはカーネルだけで、ユーザプロセスはシングルスレッドです |
| **`poll` / `select`** | なし。複数のディスクリプタを同時に待つことはできません |
| **ターミナル** | `termios` (カノニカル/raw、エコー、シグナル文字) と `ioctl` はあります。セッション、プロセスグループ、ジョブ制御、pty、`/dev/tty` はありません |
| **シェル** | 分割済み。`/bin/msh` はring 3で動きますが、大半のコマンドはカーネル内シェルに残っています。syscallに相当するものがないカーネル内部を直接呼ぶためです |
| **TLS** | カーネル内に実装されています。信頼できないネットワーク入力をring 0で解析するのは本来あるべき場所ではありません |
| **テスト** | ユニットテストはありません。正しさはCIのQEMUブートスモークテストで担保しています |
| **NVIDIA** | 立ち上げ作業中: TU116/TU117とGB206のGSP-RMブート経路。実用的なディスプレイドライバではありません |

---

## カーネル

| 構成要素 | 説明 |
|:--|:--|
| **アーキテクチャ** | x86_64、`#![no_std]`、`#![no_main]` |
| **ブート** | GRUB2 + Multiboot2、BGR/RGB自動判別のフレームバッファ |
| **アドレス空間** | 上位半分のカーネル `0xFFFFFFFF80000000`、HHDM直接マップ `0xFFFF800000000000` |
| **保護** | GDT + TSS + IST (double fault、page fault、GPF)、ring 0 / ring 3 |
| **割り込み** | IDT: タイマ、キーボード、ATA IRQ 14/15、NMI、MCE、#UD、#NM、#PF、#GP、double fault、LAPICエラー、spurious、IPI 3ベクタ、MSI 16ベクタ |
| **割り込みコントローラ** | LAPIC + IO-APIC。レガシー8259は初期化後に全マスク |
| **タイマ** | LAPICタイマ、250 Hz、PITチャネル2で校正し範囲チェックとフォールバックあり |
| **SMP** | 最大64 CPU (`MAX_CPUS`)、ACPI MADT列挙、INIT/SIPI起動、CPUごとのGDT/TSS/GS |
| **スケジューリング** | CFS方式のvruntime、CPUごとのランキュー、ワークスティーリング、プリエンプティブタイマ |
| **SSE** | CR0.EM=0、CR0.MP=1、CR4.OSFXSR=1、CR4.OSXMMEXCPT=1 |
| **カーネルヒープ** | 128 MiB、`.bss` 上の静的領域、`linked_list_allocator` |
| **カーネルスタック** | BSP 1 MiB、スレッドごと512 KiB、APごと64 KiB、すべてガードページ付き |
| **システムコール** | 72個 (0-71)、`SYSCALL`/`SYSRET` MSR経由、naked asmブリッジ |
| **リアルタイムクロック** | 起動時にCMOS/MC146818を読み、ネットワーク確立後にNTPで補正 |

### ブートシーケンス

各ステップはシリアルとフレームバッファに `[boot] ok <名前>` として出力されます:

```
Physical memory manager -> ACPI (RSDP/MADT) -> APIC -> IO-APIC -> LAPIC timer ->
Real-time clock -> IRQ routing -> Virtual file system -> Shared library cache ->
Block device probe -> Block device nodes (/dev) -> Network subsystem ->
Firmware store -> NVIDIA GPU probe -> Scheduler -> Firmware SMI silence ->
PS/2 keyboard -> Interrupts -> Timer calibration -> SMP (AP bringup) ->
mikuD init daemon
```

### カーネル自身のメモリ保護

いずれも実際に発見が難しいバグを捕まえたために存在しています:

- **スタックガードページ** (`kernel/mm/kstack.rs`): 各スレッドスタックは `ページ数 + 1` の連続
  フレームを確保し、最下位の1枚をHHDMからアンマップします。オーバーフローはガードページで
  フォルトし、その命令の位置で止まり、ハンドラが所有pidを示します。カーネルイメージは1 GiBの
  ヒュージページ1枚でマップされているため、これがないとオーバーフローはアロケータが次に渡した
  領域を黙って破壊します。
- **BSPスタックカナリア** (`kernel/kcore/stack_guard.rs`): ブートスタック直下に毒入りページを置き、
  各初期化ステップの後に検査します。
- **`IrqMutex`** (`kernel/kcore/irq_lock.rs`): クリティカルセクション中は割り込みを無効化します。
  割り込みハンドラが触れうるロック (`PMM`、`SWAP_MAP`、`EMERGENCY_POOL`、`REFCOUNTS`、
  `MSI_HANDLERS`) には必須です。通常のスピンロックではCPUが自分自身に対してデッドロックします。
- **`SchedMutex`** (`kernel/kcore/sched_lock.rs`): 短時間スピンした後スケジューラにCPUを譲ります。
  ディスクI/Oをまたいで保持されるVFSロックに使います。そこでのスピンはタイムスライスを丸ごと
  浪費します。

---

## メモリ管理

### 物理メモリ (`kernel/mm/pmm.rs`)

ワード単位で走査するビットマップフレームアロケータ。単一フレーム用と連続確保用で別々のヒントを
持ち、コピーオンライト用の参照カウントと、スワップイン経路が使う緊急予備プールを備えます。

### 仮想メモリ (`kernel/mm/vmm.rs`)

`AddressSpace` 型の背後にある4レベルページング。ユーザアドレス空間はカーネルのテーブルから
PML4エントリ256-511をコピーするため、カーネルとHHDMのマッピングは共有され食い違いません。
4 KiBマッピングが必要な場合 (MMIO、ガードページ) はヒュージページを必要に応じて分割します。

### スワップ (`kernel/mm/swap.rs`、`swap_map.rs`)

物理フレームから `(cr3, virt)` への逆マッピング、エージングとピン留めを伴うクロックスイープ方式の
追い出し、元のフラグを保持するスワップPTEエンコーディング。回収は **kswapd** スレッドで行います。
タイマティックはフラグを立てるだけです。追い出しはディスクI/Oを行い、割り込みハンドラから触れて
はいけないロックを取るためです。

### mmap (`kernel/mm/mmap.rs`)

匿名マッピングとファイルマッピング、デマンドページング、`fork` をまたぐコピーオンライト、
`mprotect`、`msync`。

---

## プロセスとスケジューリング

| 機能 | 詳細 |
|:--|:--|
| **モデル** | スレッドごとに1つの `Process`。カーネルスレッドはカーネルCR3を共有し、ユーザプロセスは自身のものを持ちます |
| **生成** | `fork` (CoW)、`exec`/`execve`、`wait4`、`kill`、ゾンビ回収 |
| **スケジューラ** | 優先度重み付きのCFS方式vruntime、CPUごとのランキュー、最小vruntime選択 |
| **負荷分散** | 最も空いている対象CPUへの配置と、キューが空いたときのワークスティーリング |
| **プリエンプション** | タイマ駆動。nakedスタブがスケジューラの期待する15 GPR + iretフレームを構築し、返された任意のスタックで実行を継続します |
| **アフィニティ** | プロセスごとの64ビットCPUマスク |
| **シグナル** | ユーザ登録のディスパッチエントリ、`sigreturn`、SIGINT/SIGQUIT/SIGTERM/SIGKILL/SIGCHLD |
| **ワーカプール** | ACPIのCPU数から算出し4-32に制限 |
| **ディスクリプタ** | プロセスごとのFDテーブル。`fork` で複製され、回収時ではなく `exit` 時に解放されます |

---

## システムコール

72エントリ。`gs` を使ってCPUごとのカーネルスタックに切り替えるnaked `SYSCALL` ブリッジから
ディスパッチされます。

| 範囲 | 領域 |
|:--|:--|
| 0-10 | `exit`、`write`、`read`、`mmap`、`munmap`、`mprotect`、`brk`、`getpid`、`getcwd`、`set_tls`、`get_tls` |
| 11-17 | `open`、`close`、`seek`、`fsize`、`map_lib`、`sleep`、`uptime` |
| 18-27 | `stat`、`fstat`、`mkdir`、`rmdir`、`unlink`、`readdir`、`rename`、`link`、`chmod`、`chown` |
| 28-42 | `dup`、`dup2`、`truncate`、`write_file`、`symlink`、`readlink`、`pipe`、`chdir`、`statfs`、`fallocate`、`getxattr`、`setxattr`、`utimensat`、`fsync`、`punch_hole` |
| 43-47 | `fork`、`wait4`、`kill`、`exec`、`umask` |
| 48-55 | `getuid`、`getgid`、`geteuid`、`getegid`、`setuid`、`setgid`、`seteuid`、`setegid` |
| 56-66 | `socket`、`connect`、`send`、`recv`、`mmap_file`、`msync`、`bind`、`listen`、`accept`、`sendto`、`recvfrom` |
| 67-69 | `execve`、`sigentry`、`sigreturn` |
| 70-71 | `clock_gettime`、`ioctl` |

**ユーザポインタ検証** (`kernel/syscall/usercopy.rs`): 範囲全体がカノニカルなユーザ半分に収まって
いること、各ページが `PRESENT | USER` (必要なら `WRITABLE`) であることを走査して確認し、遅延VMA
ページを先にフォールトインさせ、パスは使用前にカーネルメモリへコピーします。検証後に書き換え
られないようにするためです。

---

## 仮想ファイルシステム

| 項目 | 値 |
|:--|:--|
| vnode | 256 |
| プロセスあたりのオープンファイル | 128 |
| マウントスロット | 8 (VFS) / 4 (extドライバ) |
| 名前長 | 64バイト |
| ページキャッシュ | 1024ページ × 512 B = 512 KiB |
| ファイルシステム種別 | tmpfs、devfs、procfs、ext2、ext3、ext4、cowfs、pipefs |

機能: 階層的な名前空間、ハードリンクとシンボリックリンク、uid/gid/umaskによる権限、拡張属性、
ファイルロック、プロセスごとのcwd、dentryキャッシュ、ジャーナルフック、クォータ集計、
内容アドレス指定ストレージの補助機能、ブロック層から生成される `/dev` のブロックデバイスノード。

`/proc` は `uptime`、`meminfo`、`diskstats` などを提供し、`/dev` にはコンソール、null、zero、random、
ブロックデバイスがあり、`/lib` には事前ロードされた10個の共有ライブラリが置かれます。

---

## ファイルシステム

### ext2 / ext3 / ext4 (`kernel/fs/ext/`)

ドライバはスーパーブロックの機能ビットからディスク上の版を判別し、そのとおりに `ext2`、`ext3`、
`ext4` と報告します。マウントログ、`statfs()`、`/proc` のいずれでも同じです:

```
[miku_extfs] slot 0 drive 1 lba 0 - ext4 (journal=true extents=true 64bit=true
                                          metadata_csum=true flex_bg=true dir_index=true)
```

| 機能 | 詳細 |
|:--|:--|
| **読み込み** | 直接、間接、二重間接、三重間接ブロック、ext4のextentツリー、インラインデータ |
| **書き込み** | グループごとのヒントを用いた割り当て、truncate、punch hole、fallocate、rename、リンク |
| **ext3** | JBD2形式のジャーナル: トランザクション、ディスクリプタブロックとrevokeブロック、マウント時のリプレイ |
| **ext4** | extentツリー、64ビットブロック番号、flex_bg、メタデータチェックサム (CRC32c)、インラインデータ |
| **ボリュームサイズ** | ドライバではなくファイルシステム側の制限。ブロックグループテーブルはヒープ上で伸長します |
| **整合性** | fsck、マウント時の孤立inode整理、TRIM/discard、`fiemap` |
| **mkfs** | ext2/ext3/ext4の作成、ドライラン対応 |

### その他

- **tmpfs** ルートファイルシステム、ページキャッシュ上
- **devfs**、**procfs** 合成生成
- **GPT** パーティションテーブル解析、パーティションノード、`partprobe`

---

## ストレージスタック

```
VFS  ->  ブロック層  ->  バッファキャッシュ  ->  ドライバ  ->  デバイス
```

- **ブロック層** (`kernel/io/block/`): デバイスレジストリ、パーティションマッピング、デバイス単位の
  I/O統計、discardとwrite-zeroes、ヘルス監視 (NVMeログページとATA SMART)、`/proc/diskstats`
- **バッファキャッシュ**: 4 KiBチャンク512個、8ウェイセットアソシアティブ、**bdflush** フラッシャ
  スレッドによるライトバック
- **キュー**: `&self` のドライバとロックフリーなディスパッチ。NVMeはCPUごとに4本の送信キューを使用

**ドライバ** (`kernel/drivers/block/`): `ata` (PIO + バスマスタDMA)、`ahci`、`nvme`、`virtio_blk`、
`ramdisk` (GRUBから渡されるファームウェアイメージ用)。

---

## ネットワークスタック

| 層 | 対応 |
|:--|:--|
| **ドライバ** | Intel e1000 (82540EM/82545EM/82574L/82579LM/I217)、Realtek RTL8139/8168/8169、virtio-net |
| **リンク** | Ethernet、キャッシュ付きARP |
| **ネットワーク** | IPv4、ICMP (ping、traceroute) |
| **トランスポート** | UDP、完全な状態機械と再送、リスナを備えたTCP |
| **アプリケーション** | DHCPクライアント、DNSリゾルバ、HTTP/1.1、HTTP/2、NTP |
| **セキュリティ** | TLS 1.2/1.3クライアント: RSA、ECDHE、AES-GCM、多倍長演算。現状ring 0で動作 |
| **ソケット** | プロセスごとのソケットテーブル、`socket`/`bind`/`listen`/`accept`/`connect`/`send`/`recv`/`sendto`/`recvfrom` |

`netd` は起動時にDHCPを実行し、リンクを自動で確立します。

---

## 入力とコンソール

- **PS/2キーボード**: コントローラ初期化、スキャンコードセット1、ロックフリーリングバッファ
- **USB**: xHCIコントローラ、HIDキーボード、BIOS/EHCIからのハンドオフ
- **コンソール**: 色、スクロール、ビルド時に生成されるJetBrains Monoビットマップフォントを備えた
  フレームバッファテキストコンソール。コンソール出力はすべてシリアルにミラーされます。これが
  ヘッドレス起動とCIログを意味あるものにしています
- **ターミナル** (`kernel/io/input/user_stdin.rs`): カノニカルモードとrawモード、エコー制御、
  Ctrl-CでSIGINT、Ctrl-\でSIGQUIT、Ctrl-DでEOF、Ctrl-Uで行削除。設定はring 3から `ioctl`
  (`TCGETS`、`TCSETS`、`TIOCGWINSZ`) で操作できます

---

## mikuD initデーモン

PID 1。サービスのライフサイクル、依存関係、再起動を管理します。

| 概念 | 値 |
|:--|:--|
| **ターゲット** | `SysInit`、`MultiUser`、`Graphical`、`Rescue` |
| **再起動ポリシー** | `Always`、`Never`、`OnFailure`、`OnSuccess`、`OnAbnormal` |
| **サービスエントリ** | カーネルの `fn()` またはディスク上のELFバイナリパス |
| **その他** | 依存グラフ、順序制御、ウォッチドッグ、再起動遅延、マスク、ジャーナル、タイマユニット、ソケットアクティベーション、`.service` ユニットファイル |

起動時に登録されるサービス: `kbd`、`shell`、`netd`、`usbd`、`bdflush`、`kswapd`。

`shell` サービスはルートファイルシステムに `/bin/msh` があればそれ (ring 3) を優先し、なければ
カーネル内シェルにフォールバックします。どちらを選んだかはログに出ます。

---

## ユーザ空間

### 動的リンク

`ld-miku` が動的ローダです: ELF64解析、`PT_LOAD` マッピング、再配置処理 (`RELA`、`JMPREL`、
`GLOB_DAT`、`JUMP_SLOT`、`RELATIVE`)、ライブラリ間のシンボル解決、`DT_NEEDED` 依存の走査、
TLS設定、完全なauxv。

10個の共有ライブラリがVFSの `/lib` に事前ロードされ、ページキャッシュを介さずカーネルイメージの
メモリから直接提供されます:

`core_miku`、`sys_miku`、`text_miku`、`ds_miku`、`algo_miku`、`codec_miku`、`fs_miku`、`net_miku`、
`parse_miku`、`libc_miku`

同じライブラリをマップするプロセス間でページは共有され、書き込み可能セグメントのみが専用になります。

### `/bin/msh` ユーザ空間シェル

syscall ABIだけの上に構築されたring 3のシェルです。これがABIを正直に保っています。

**組み込みコマンド:** `pwd`、`cd`、`ls`、`cat`、`stat`、`mkdir`、`rm`、`write`、`echo`、`wc`、
`head`、`grep`、`uptime`、`date`、`stty`、`pid`、`help`、`exit`

**シェル機能:** リダイレクト `>` `>>` `<`、パイプライン `|` (ステージごとにfork)、起動スクリプト
`/etc/msh.rc`。

```
$ cat /p.txt | wc
3 3 14
$ ls /bin | grep msh
msh
```

### ユーザ空間プログラムのビルド

```bash
cd src/lib/userspace
cargo +nightly build --release --target x86_64-miku-app.json \
    -Z json-target-spec -Z build-std=core \
    -Z build-std-features=compiler-builtins-mem --bin msh
```

builderはこれを自動で行い、バイナリをルートイメージの `/bin` に配置します。

---

## カーネル内シェル

依然として2つのシェルのうち大きい方です。これらのコマンドはsyscallが存在しないカーネル内部に
アクセスするためです。およそ190コマンド:

- **ファイル**: `ls`、`cat`、`cd`、`cp`、`mv`、`rm`、`mkdir`、`tree`、`du`、`stat`、`ln`、`chmod`、`chattr`
- **ext (版に依存しない)**: `extls`、`extcat`、`extwrite`、`extfsck`、`extinfo`、`extsync` と
  `ext2*` / `ext3*` / `ext4*` 系
- **マウント**: `mount`、`umount`、`fs.list`、`fs.select`、`partprobe`、`gpt`、`mkfs.ext2/3/4`
- **ストレージ**: `blkstat`、`blkdiscard`、`blkzero`、`smart`、`fstrim`、`fiemap`、`nvmestress`
- **スワップ**: `mkswap`、`swapon`、`swapoff`、`swapinfo`
- **プロセス**: `ps`、`top`、`kill`、`nice`、`affinity`、`exec`
- **サービス**: `sv start|stop|restart|status|enable|disable|mask|journal|timer|analyze`
- **ネットワーク**: `net`、`ping`、`dhcp`、`ntp`、`wget`、`curl`、`fetch`、`traceroute`、`socket`
- **リンク**: `ldd`、`ldconfig`、`load`
- **GPU**: `nvidia` のサブコマンド
- **システム**: `info`、`heap`、`memmap`、`history`、`reboot`、`poweroff`

---

## NVIDIA GPUドライバ

立ち上げ作業中であり、ディスプレイドライバではありません。

| チップ系列 | 対象 | 状態 |
|:--|:--|:--|
| TU116 / TU117 (Turing) | GTX 1650 / 1660 | Falcon起動、FWSEC、ACR、SEC2、GSP-RMブート引数、メッセージキュー |
| GB206 (Blackwell) | RTX 5060 / 5060 Ti | FSP、FMC、GSPブートローダ経路 |

モジュール構成 (`kernel/drivers/gpu/nvidia/`): `pci`、`mmio`、`chip`、`vbios`、`reset`、`msi`、`fb`、
`profile`、`generic`、共通の `gsp_common/` (RPC、sysinfo)、チップ別の `gtx1650/` と `rtx5060/`。

ファームウェアはカーネルに埋め込まれていません。`/lib/firmware` ツリーに配置し、ext2イメージに
まとめ、GRUBモジュールとしてカーネルに渡して必要時にマウントします。そのため1枚のISOがカーネルと
ファームウェアの両方を運べます。

**NVIDIAのブロブはリポジトリに含まれていません。** ベンダの署名済み専有マイクロコードだからです。
ブロブなしでもビルドは動作します。ファームウェアイメージが作られず、カーネルは
`firmware unavailable` と報告するだけです。ローカルで追加する方法は
`kernel/drivers/gpu/nvidia/firmware/README.md` にあります。

---

## ビルドと実行

### 必要なもの

| ツール | 用途 |
|:--|:--|
| Rust nightly + `rust-src` | ベアメタルターゲット向けの `build-std` |
| `grub-mkrescue`、`xorriso`、`mtools` | ISO作成 |
| `qemu-system-x86_64` | 実行 |
| `e2fsprogs` (`mke2fs`、`debugfs`) | ルートイメージとファームウェア配置 |

### すべてビルド

```bash
cd builder
cargo run
```

builderは `ld-miku`、mikuライブラリ、ユーザ空間プログラム、カーネル (release) をビルドし、ISOを
作成し、`/lib/firmware` と `/bin` を含む `disk.img` を用意し、QEMUを起動できます。

### 手動で実行

```bash
qemu-system-x86_64 \
  -boot d -cdrom miku-os/miku-os.iso \
  -drive file=miku-os/disk.img,format=raw,if=none,id=disk0,cache=unsafe,aio=threads \
  -device ide-hd,drive=disk0,bus=ide.0,unit=1 \
  -serial stdio -display gtk -m 4G -smp 4 \
  -device qemu-xhci,id=xhci -device usb-kbd,bus=xhci.0
```

### USBメモリへの書き込み

```bash
sudo dd if=miku-os/miku-os.iso of=/dev/sdX bs=4M status=progress conv=fdatasync
```

---

## リポジトリ構成

```
kernel/                 カーネルソース
  arch/x86_64/          ブートエントリ、GDT、IDT、APIC、ACPI、SMP、RTC、per-CPU、シリアル
  mm/                   pmm、vmm、ヒープ、mmap、スワップ、カーネルスタック
  sched/                CFSスケジューラ、ランキュー、ライフサイクル、ワーカ
  process/              Process、ELFローダ、動的リンク、シグナル
  syscall/              ディスパッチ、アダプタ、usercopy、errno
  fs/                   VFS、ext2/3/4、tmpfs、devfs、procfs、mkfs、GPT
  io/                   ブロック層、コンソール、フレームバッファ、入力
  net/                  Ethernetから TLSまで
  drivers/              block、bus (PCI)、net、input (PS/2、USB)、gpu (NVIDIA)
  mikud/                initデーモン
  shell/                カーネル内シェル
  kcore/                ブート状態、クロック、ファームウェア、ロック、電源、RNG、時間
src/lib/                ユーザ空間
  ld_miku/              動的ローダ
  libmiku/、mikulibs/   標準ライブラリのソースとドメイン別ライブラリ
  userspace/            ring 3プログラム (msh、hello、テスト)
builder/                ビルド統括とISO/ディスク作成
docs/                   翻訳、ABI仕様、GPUメモ
```

---

## 作者

**alunwrd** [github.com/alunwrd](https://github.com/alunwrd)

Rustでゼロから、一人で書いています。

## ライセンス

MIT、[LICENSE](../LICENSE) を参照してください。
