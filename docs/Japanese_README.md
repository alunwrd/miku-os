<div align="center">

# Miku OS

**Rustで開発された実験的なオペレーティングシステムカーネル**

*Rustと数人の開発者によって動いています :D*

<img src="https://raw.githubusercontent.com/alunwrd/miku-os/main/docs/miku.png" width="220" alt="Miku Logo">

[![Language](https://img.shields.io/badge/language-Rust-orange.svg)](https://www.rust-lang.org/)
[![Architecture](https://img.shields.io/badge/arch-x86__64-blue.svg)]()
[![Status](https://img.shields.io/badge/status-release-green.svg)]()
[![License](https://img.shields.io/badge/license-MIT-lightgrey.svg)]()

</div>

---

> **ドキュメント:** [Russian](Russian_README.md) | [English](English_README.md) | [Japanese](Japanese_README.md)

---

## プロジェクトについて
**Miku OS** は `no_std` 環境でゼロから開発されたオペレーティングシステムです。
標準ライブラリ (`libc`) を一切使用せず、ハードウェアとメモリアーキテクチャを完全に制御します。
ELFダイナミックリンク、共有ライブラリ、ユーザースペースプロセス、initデーモン (mikuD)、
プロセス管理 (fork/exec/wait)、シグナルを独自実装で実現しています。

> すべてのコードはRustで書かれています。アセンブラはブートローダー、syscallハンドラー、コンテキストスイッチの部分にのみ使用しています。

---

## 技術仕様

### カーネル

| コンポーネント | 説明 |
|:--|:--|
| **アーキテクチャ** | x86_64、`#![no_std]`、`#![no_main]` |
| **ブートローダー** | GRUB2 + Multiboot2、フレームバッファ (BGR/RGB 自動検出) |
| **保護機能** | GDT + TSS + IST (ダブルフォルト、ページフォルト、GPF用)、ring 0 / ring 3 |
| **割り込み** | IDT: タイマー、キーボード、ページフォルト、GPF、#UD、#NM、ダブルフォルト |
| **PIC** | PIC8259 (オフセット 32/40) |
| **SSE** | CR0.EM=0、CR0.MP=1、CR4.OSFXSR=1、CR4.OSXMMEXCPT=1 |
| **ヒープ** | 32 MB、リンクリストアロケータ |
| **Syscall** | MSR経由のSYSCALL/SYSRET、naked asmハンドラー、R8/R9/R10保存 |
| **シグナル** | SIGKILL (9)、SIGTERM (15)、SIGCHLD (17)、32ビットビットマスク |
| **Init** | mikuD (PID 1) - systemd風サービス管理デーモン |

---

### mikuD - Initデーモン

<details>
<summary><b>展開する</b></summary>

#### 概要

mikuDはMikuOSのinitデーモン (PID 1) です。systemd風のサービスライフサイクル管理、
依存関係解決、ターゲット (ランレベル)、ウォッチドッグ、通知、ソケットアクティベーション、
タイマー、ELFバイナリの起動、グレースフルシャットダウンをサポートします。

#### ターゲット (ランレベル)

| ターゲット | 値 | 説明 |
|:--|:--:|:--|
| **SysInit** | 0 | システム初期化 |
| **MultiUser** | 1 | マルチユーザーモード (デフォルト) |
| **Graphical** | 2 | グラフィカルモード |
| **Rescue** | 3 | レスキュー / シングルユーザーモード |

ターゲット遷移時にサービスの自動起動・停止が行われます。

#### サービスタイプ

| タイプ | 説明 |
|:--|:--|
| **Simple** | 長期実行サービス (デフォルト) |
| **Oneshot** | 一度実行して完了 |
| **Notify** | `notify_ready()` で準備完了を通知 |
| **Forking** | 子プロセスをフォーク |

#### リスタートポリシー

| ポリシー | 動作 |
|:--|:--|
| **Always** | 常にリスタート |
| **Never** | リスタートしない |
| **OnFailure** | exit code != 0 のときのみ |
| **OnSuccess** | exit code == 0 のときのみ |
| **OnAbnormal** | シグナルまたは非ゼロ終了時 |

#### 依存関係タイプ

| タイプ | 動作 |
|:--|:--|
| **Requires** (deps) | ハード依存 - 依存先が失敗するとサービスも失敗 |
| **Wants** | ソフト依存 - 依存先が失敗しても続行 |
| **Conflicts** | 起動前に競合するサービスを停止 |

#### 機能

| 機能 | 詳細 |
|:--|:--|
| **ExecStart** | ディスク上のELFバイナリをサービスとして起動 |
| **ウォッチドッグ** | タイムアウト内にpingが必要、なければリスタート |
| **Notify** | sd_notify相当 - サービスが準備完了を通知 |
| **条件** | ConditionPathExists、ConditionServiceActive、ConditionTargetActive |
| **マスキング** | サービスの起動を完全に禁止 |
| **クリティカル** | 保護されたサービスはユーザーが停止できない |
| **バースト保護** | 10秒間に最大5回のリスタート制限 |
| **グレースフルシャットダウン** | 非クリティカル→クリティカルの順に停止、30秒タイムアウト |
| **ブート分析** | ブート時の全サービスのタイミングデータ |
| **環境変数** | サービスあたり最大8つの key=value ペア |
| **タイムアウト** | 起動/停止タイムアウト設定可能 (デフォルト10秒) |
| **リスタートフック** | サービス再起動前のコールバック |
| **Isolate** | ターゲットを切り替え、不要なサービスを停止 |

#### ジャーナル (イベントログ)

128エントリのリングバッファに全てのmikuDイベントを記録:

| イベント | シンボル | 説明 |
|:--|:--:|:--|
| Started | + | サービス起動 |
| Stopped | - | サービス停止 |
| Exited | x | サービス終了 (終了コード付き) |
| Failed | ! | サービス失敗 |
| DepFailed | d | 依存関係の失敗 |
| ExecFailed | E | ELFバイナリの起動失敗 |
| Reloaded | R | SIGHUPリロード |
| WatchdogTimeout | W | ウォッチドッグ期限切れ |
| BurstLimit | B | リスタートレート制限到達 |
| Shutdown | S | グレースフルシャットダウン開始 |
| TimerFired | F | タイマー発火 |
| SocketActivated | A | ソケットアクティベーション |

イベントには重要度レベルがあります: info (0)、notice (1)、warning (2)、critical (3)。

#### タイマーユニット

| タイプ | 動作 |
|:--|:--|
| **Interval** | N tick ごとに繰り返し発火 |
| **Oneshot** | N tick 後に一度だけ発火、その後無効化 |
| **Realtime** | ブート時間に基づいて N tick ごとに発火 |

最大16タイマー。タイマー発火でサービスを起動します。

#### ソケットアクティベーション

登録されたポートに接続が来た時にサービスをオンデマンドで起動します。
Stream (TCP) と Dgram (UDP) のソケットタイプをサポート。最大16ソケット。

#### ユニットファイル (.service)

`/etc/mikud/` からロードされるINI形式:

```ini
[Unit]
Description=My service
After=kbd network
Wants=logging
Conflicts=rescue-shell
ConditionPathExists=/etc/config

[Service]
Type=simple
ExecStart=/usr/bin/myservice
Restart=always
RestartSec=50
Priority=5
WatchdogSec=100
TimeoutStartSec=2500
RemainAfterExit=false
Critical=false
Environment=LANG=en

[Install]
WantedBy=multi-user
```

#### シェルコマンド (sv)

| コマンド | 説明 |
|:--|:--|
| `sv list` | 全サービスの一覧 (状態、PID、リスタート回数) |
| `sv status <name>` | 詳細なステータス + ジャーナルエントリ |
| `sv start <name>` | サービスを起動 |
| `sv stop <name>` | サービスを停止 (グレースフル) |
| `sv restart <name>` | サービスを再起動 |
| `sv reload <name>` | SIGHUP送信 (設定リロード) |
| `sv enable <name>` | サービスを有効化 |
| `sv disable <name>` | サービスを無効化 (停止 + 非アクティブ化) |
| `sv mask <name>` | サービスの起動を禁止 |
| `sv unmask <name>` | マスクされたサービスの起動を許可 |
| `sv force-stop <name>` | 強制終了 (クリティカルサービスも) |
| `sv journal [name]` | イベントログ (直近20件またはサービス別) |
| `sv target [name]` | アクティブターゲットの表示/設定 |
| `sv isolate <tgt>` | ターゲット切り替え、不要サービス停止 |
| `sv analyze` | ブートタイミング分析 |
| `sv tree <name>` | 依存関係ツリー |
| `sv rdeps <name>` | 逆依存関係 |
| `sv cat <name>` | サービスのユニット設定表示 |
| `sv load <path>` | .serviceユニットファイルをロード |
| `sv scan` | /etc/mikud/ のユニットファイルをスキャン |
| `sv timer list` | タイマーユニット一覧 |
| `sv timer start/stop <name>` | タイマー制御 |

</details>

---

### ELFローダーとダイナミックリンク

<details>
<summary><b>ELFローダー</b></summary>

#### 機能

| 機能 | 説明 |
|:--|:--|
| **対応形式** | ET_EXEC (静的)、ET_DYN (PIE) |
| **セグメント** | PT_LOAD、PT_INTERP、PT_DYNAMIC、PT_TLS、PT_GNU_RELRO、PT_GNU_STACK |
| **リロケーション** | R_X86_64_RELATIVE、R_X86_64_JUMP_SLOT、R_X86_64_GLOB_DAT、R_X86_64_64 |
| **セキュリティ** | W^X enforcement (W+Xセグメント拒否)、RELRO |
| **ASLR** | PIEバイナリに20ビットエントロピー (RDRAND + TSCフォールバック) |
| **スタック** | SysV ABI準拠: argc、argv、envp、auxv (16バイトアラインメント) |
| **TLS** | Thread Local Storage (FS.baseレジスタ経由) |

#### モジュール構成

| モジュール | 説明 |
|:--|:--|
| **elf_loader.rs** | ELFパース、セグメントマッピング |
| **exec_elf.rs** | プロセス生成、スタック構築 |
| **dynlink.rs** | ダイナミックリンク (reloc.rsに委譲) |
| **reloc.rs** | 統合リロケーションエンジン |
| **vfs_read.rs** | 統合ファイル読み込み (VFS + ext2) |
| **random.rs** | RDRAND/TSC乱数、ASLR |

#### auxvエントリ

| キー | 説明 |
|:--|:--|
| AT_PHDR | プログラムヘッダーの仮想アドレス |
| AT_PHENT | プログラムヘッダーのエントリサイズ |
| AT_PHNUM | プログラムヘッダーの数 |
| AT_PAGESZ | ページサイズ (4096) |
| AT_ENTRY | 実行ファイルのエントリポイント |
| AT_BASE | インタープリターのベースアドレス |
| AT_RANDOM | 16バイトのランダムデータ |

</details>

<details>
<summary><b>ld-miku (ダイナミックリンカー)</b></summary>

#### 概要

`ld-miku` はMikuOS用のELFダイナミックリンカーです。Rustで `#![no_std]` 環境で書かれ、
静的PIEバイナリとしてコンパイルされます。

#### 処理フロー

```
1. カーネルがELFをロード → PT_INTERPを検出
2. ld-miku.soをINCLUDE_BYTESからメモリにマッピング
3. ld-miku起動 → auxvからAT_PHDR/AT_ENTRYを解析
4. DT_NEEDEDから必要なライブラリを特定
5. SYS_MAP_LIB syscallで共有ライブラリをマッピング
6. PLT/GOTリロケーションを適用
7. シンボルをグローバルテーブルにエクスポート
8. DT_INIT / DT_INIT_ARRAYを実行
9. 実行ファイルのエントリポイントにジャンプ
```

#### 特徴

- グローバルシンボルテーブル (最大1024シンボル)
- weakシンボルの解決
- 再帰的な依存ライブラリのロード (最大16ライブラリ)
- R_X86_64_COPY リロケーション対応
- DT_HASH / DT_GNU_HASH によるシンボル数の正確な取得
- envp を正しくスキップするauxv解析

</details>

<details>
<summary><b>共有ライブラリ (solib)</b></summary>

#### グローバルライブラリキャッシュ

| パラメータ | 値 |
|:--|:--|
| **最大キャッシュ数** | 32ライブラリ |
| **検索パス** | /lib、/usr/lib |
| **ページマッピング** | 全セグメントをプロセスごとにコピー |
| **OOM保護** | parse_and_prepare中のOOMで部分キャッシュを防止 |

#### SYS_MAP_LIB syscall (nr=15)

カーネルがELFセグメントを解析し、共有ライブラリを直接プロセスのアドレス空間にマッピングします。

- read-onlyセグメント → キャッシュからプライベートコピー
- writableセグメント → プロセスごとに新規アロケーション
- map_page失敗時のロールバック対応

#### システムライブラリ

`libmiku.so` は `include_bytes!` でカーネルに組み込まれ、`solib::preload` で起動時にキャッシュに登録されます。

#### シェルコマンド

| コマンド | 説明 |
|:--|:--|
| `ldconfig` | /lib と /usr/lib をスキャンしキャッシュを更新 |
| `ldd` | キャッシュされたライブラリの一覧表示 |

</details>

---

### libmiku.so (標準ライブラリ)

<details>
<summary><b>展開する</b></summary>

#### 概要

libmikuはMikuOS用のC互換標準ライブラリです。Rustで書かれ、63モジュール、962関数をエクスポートします。
ld-mikuによって動的にロードされ、全てのuserspace プログラムが使用します。
POSIX libc互換レイヤー (stdio、stdlib、string.h等) を含みます。

#### 拡張ライブラリモジュール (63個)

| カテゴリ | モジュール |
|:--|:--|
| **データ構造** | vec、list、hashmap、treemap、trie、queue、ringbuf、ringbuf2、heap_queue、bitset、slab、pool |
| **文字列** | string、strbuf、ctype、utf8、format、regex、glob |
| **I/O** | io、bufio、stdio、file、dir、path |
| **数値/数学** | num、math、random、convert、endian、bitops |
| **エンコーディング** | base64、hex、json、csv、ini、lz |
| **暗号** | sha256、checksum、hash、uuid |
| **システム** | sys、proc、signal、env、errno、args、getopt |
| **並行処理** | sync、channel、event、timer |
| **時間** | time、datetime |
| **メモリ** | mem、heap、arena、pool、slab |
| **ログ/テスト** | log、test |
| **ソート** | sort |
| **libc互換** | libc (fopen/fclose/fread/fwrite/fprintf/fgets/fputs等 151関数) |

#### モジュール: io (入出力)

| 関数 | 説明 |
|:--|:--|
| `miku_write(fd, buf, len)` | fdへの書き込み |
| `miku_read(fd, buf, len)` | fdからの読み込み |
| `miku_print(str)` | 文字列出力 |
| `miku_println(str)` | 文字列出力 + 改行 |
| `miku_puts(str)` | println互換 |
| `miku_putchar(c)` | 1バイト出力 |
| `miku_getchar()` | 1バイト入力 |
| `miku_readline(buf, max)` | 行入力 (固定バッファ) |
| `miku_getline()` | 行入力 (malloc、free必要) |

#### モジュール: string (文字列)

| 関数 | 説明 |
|:--|:--|
| `miku_strlen` | 文字列長 |
| `miku_strcmp` / `miku_strncmp` | 文字列比較 |
| `miku_strcpy` / `miku_strncpy` | 文字列コピー |
| `miku_strcat` / `miku_strncat` | 文字列連結 |
| `miku_strchr` / `miku_strrchr` | 文字検索 |
| `miku_strstr` | 部分文字列検索 |
| `miku_strdup` | 文字列複製 (malloc) |
| `miku_toupper` / `miku_tolower` | 大文字/小文字変換 |
| `miku_isdigit` / `miku_isalpha` / `miku_isalnum` / `miku_isspace` | 文字分類 |
| `miku_strtok` | トークン分割 (stateful) |
| `miku_strpbrk` | 文字セット検索 |
| `miku_strspn` / `miku_strcspn` | プレフィックス長 |
| `miku_strtol` / `miku_strtoul` | 文字列→数値 (base 0/8/10/16) |
| `miku_strlcpy` / `miku_strlcat` | BSD安全コピー/連結 |

#### モジュール: num (数値)

| 関数 | 説明 |
|:--|:--|
| `miku_itoa(val, buf)` | 整数→文字列 |
| `miku_utoa(val, buf)` | 符号なし整数→文字列 |
| `miku_atoi(str)` | 文字列→整数 |
| `miku_print_int(val)` | 10進数出力 |
| `miku_print_hex(val)` | 16進数出力 (0x...) |

#### モジュール: mem (メモリ)

| 関数 | 説明 |
|:--|:--|
| `miku_memset` | メモリ塗りつぶし |
| `miku_memcpy` | メモリコピー |
| `miku_memmove` | メモリコピー (オーバーラップ対応) |
| `miku_memcmp` | メモリ比較 |
| `miku_bzero` | ゼロクリア |
| `miku_memchr` | バイト検索 |
| `miku_memrchr` | 逆方向バイト検索 |
| `miku_memmem` | バイト列検索 |

#### モジュール: heap (動的メモリ)

| 関数 | 説明 |
|:--|:--|
| `miku_malloc(size)` | メモリ確保 |
| `miku_free(ptr)` | メモリ解放 |
| `miku_realloc(ptr, size)` | サイズ変更 |
| `miku_calloc(count, size)` | ゼロ初期化確保 |

実装: mmapベースのslabアロケータ。32KB未満は128KBのslabから切り出し、32KB以上はmmap/munmapで個別管理。

#### モジュール: fmt (フォーマット出力)

| 関数 | 説明 |
|:--|:--|
| `miku_printf(fmt, ...)` | フォーマット出力 |
| `miku_snprintf(buf, max, fmt, ...)` | バッファへのフォーマット出力 |

対応フォーマット: `%s` `%d` `%u` `%x` `%c` `%p` `%%`

実装: `global_asm!` トランポリンでrsi/rdx/rcx/r8/r9をスタックに保存。XMMレジスタ不使用によりSSEアラインメント問題を回避。`%d/%x/%u` は32ビット (i32/u32として読み取り)。

#### モジュール: file (ファイルI/O)

| 関数 | 説明 |
|:--|:--|
| `miku_open(path, len)` | ファイルを開く |
| `miku_open_cstr(path)` | ファイルを開く (C文字列) |
| `miku_close(fd)` | 閉じる |
| `miku_seek(fd, offset)` | オフセット設定 |
| `miku_fsize(fd)` | ファイルサイズ取得 |
| `miku_read_file(path, &size)` | ファイル全体を読み込み (malloc) |

#### モジュール: time (時間)

| 関数 | 説明 |
|:--|:--|
| `miku_sleep(ticks)` | スリープ (~10ms/ティック) |
| `miku_sleep_ms(ms)` | ミリ秒スリープ |
| `miku_uptime()` | 起動からのティック数 |
| `miku_uptime_ms()` | 起動からのミリ秒 |

#### モジュール: proc (プロセス)

| 関数 | 説明 |
|:--|:--|
| `miku_exit(code)` | プロセス終了 |
| `miku_getpid()` | PID取得 |
| `miku_getcwd(buf, size)` | カレントディレクトリ取得 |
| `miku_brk(addr)` | ヒープ拡張 (0=クエリ) |
| `miku_mmap` / `miku_munmap` / `miku_mprotect` | メモリマッピング |
| `miku_set_tls` / `miku_get_tls` | TLSレジスタ |
| `miku_map_lib(name, len)` | 共有ライブラリのマッピング |

#### モジュール: util (ユーティリティ)

| 関数 | 説明 |
|:--|:--|
| `miku_abs` / `miku_min` / `miku_max` / `miku_clamp` | 数値ユーティリティ |
| `miku_swap(a, b)` | 値の交換 |
| `miku_srand(seed)` / `miku_rand()` / `miku_rand_range(lo, hi)` | 疑似乱数 (xorshift64) |
| `miku_assert_fail(expr, file, line)` | アサーション失敗 |
| `miku_panic(msg)` | パニック (exit 134) |

</details>

---

### ユーザースペースSDK

<details>
<summary><b>展開する</b></summary>

#### 概要

MikuOSはRust SDKを提供し、`no_std` 環境でuserspace プログラムを開発できます。
C言語も引き続きサポートされています。

#### SDK構成

```
src/lib/userspace/
├── Cargo.toml              crate設定
├── build.rs                stub libmiku.soの自動生成
├── build.sh                ビルド + デプロイスクリプト
├── x86_64-miku-app.json    ターゲット仕様
└── src/
    ├── miku.rs             SDK: externバインディング + 安全ラッパー
    ├── hello.rs            Hello Worldサンプル
    └── test_full.rs        71テスト
```

#### Rustプログラムの例

```rust
#![no_std]
#![no_main]

#[path = "miku.rs"]
mod miku;

#[no_mangle]
pub extern "C" fn _start_main() -> ! {
    miku::println("Hello MikuOS!");
    miku::exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { miku::exit(1); }
```

#### ビルドとデプロイ

```bash
cd ~/miku-os/src/lib/userspace
./build.sh hello        # ビルド + data.imgにコピー
```

#### MikuOSでの実行

```
miku@os:/ $ ext4mount 3
miku@os:/ $ exec hello
Hello MikuOS!
```

#### 安全ラッパー (miku.rs)

SDKはC ABI上に安全なRustラッパーを提供します:

| ラッパー | 説明 |
|:--|:--|
| `miku::print(s: &str)` | 文字列出力 |
| `miku::println(s: &str)` | 文字列出力 + 改行 |
| `miku::exit(code)` | プロセス終了 |
| `miku::open(path) -> Result` | ファイルオープン |
| `miku::read_file(path) -> Option` | ファイル全体読み込み |
| `miku::sleep_ms(ms)` | ミリ秒スリープ |
| `miku::rand_range(lo, hi)` | 範囲指定乱数 |
| `cstr!("text")` | C文字列マクロ |

#### エントリポイント

`_start_main` を使用します (`_start` ではない)。`miku.rs` 内の `global_asm!` トランポリンが `_start` で `and rsp, -16` によるスタックアラインメントを行い、`_start_main` を呼び出します。

#### テストスイート

1617テストが以下のカテゴリで実行されます:

| カテゴリ | テスト数 |
|:--|:--|
| strings (基本/拡張) | 24 |
| numbers | 7 |
| memory | 4 |
| utilities | 7 |
| heap | 7 |
| process | 2 |
| printf / snprintf | 11 |
| time | 5 |
| file I/O | 3+ |
| libc互換 (stdio) | 1500+ |

</details>

---

### メモリ管理

<details>
<summary><b>物理メモリ (PMM)</b></summary>

#### フレームアロケータ

- ビットマップアロケータ: 最大4Mフレーム (16 GB RAM)、1ビット = 1フレーム 4KB
- `free_hint` と `contiguous_hint` で空きフレームの検索を高速化
- 連続alloc: 1回のリクエストでNフレームをまとめて確保
- リージョン: Multiboot2メモリマップからRAM範囲を動的に登録

#### エマージェンシープール

| パラメータ | 値 |
|:--|:--|
| **プールサイズ** | 64フレーム (256 KB) |
| **用途** | ページフォルトハンドラー内のswap-inのみ |
| **補充** | `refill_emergency_pool_tick()` 経由でTimer ISRが250Hzごとに実行 |

```
alloc_frame()           - PMMからの通常alloc
alloc_frame_emergency() - エマージェンシープールのみ (フォルトハンドラー用)
alloc_or_evict()        - RAMが不足した場合にalloc + evict
alloc_for_swapin()      - エマージェンシープールのみ (faultコンテキスト)
```

</details>

<details>
<summary><b>仮想メモリ (VMM)</b></summary>

- 4レベルページテーブル (PML4 → PDP → PD → PT)
- HHDM: Higher Half Direct Map (`0xFFFF800000000000`)
- `mark_swapped()`: ページをスワップアウトした際のswap PTE書き込み
- ring 0 / ring 3 マッピングのサポート
- ユーザープロセス用アドレス空間の作成と破棄

</details>

<details>
<summary><b>mmap サブシステム</b></summary>

| パラメータ | 値 |
|:--|:--|
| **MMAP範囲** | 0x100000000 ~ 0x7F0000000000 |
| **BRK範囲** | 0x6000000000 ~ |
| **最大VMA** | 256エントリ |
| **機能** | mmap、munmap、mprotect、brk |
| **MAP_FIXED** | 既存マッピングのunmap + VMA重複除去 |
| **VMA検証** | insert失敗時のロールバック |

</details>

<details>
<summary><b>スワップ (Swap)</b></summary>

#### リバースマッピング (swap_map)

- 各物理フレームに `(cr3, virt_addr, age, pinned)` を記録
- 最大512Kフレーム (2 GB RAM) を追跡

#### 追い出しアルゴリズム: クロックスイープ

```
Pass 1: age >= 3 のフレームを検索 (最も古いもの)
Pass 2: 緊急時、unpinnedフレームを任意に取得
```

- `touch(phys)`: ページアクセス時にageを1にリセット
- `age_all()`: タイマーで全フレームのageを増加

#### Swap PTEエンコーディング

```
bit 0     = 0  (PRESENT=0)
bit 1     = 1  (SWAP_MARKER)
bits 12.. = スワップスロット番号
判定条件: slot番号が0でないことを追加検証 (false positive防止)
```

</details>

---

### プロセス管理

| 機能 | 説明 |
|:--|:--|
| **fork()** | COW方式のプロセスクローン (ページテーブルのディープコピー) |
| **exec()** | プロセスイメージをELFバイナリで置換 |
| **wait4()** | 子プロセスの待機 (ブロッキング) |
| **kill()** | プロセスへのシグナル送信 (SIGTERM、SIGKILL、SIGCHLD) |
| **ゾンビ回収** | mikuDおよびwait4による自動回収 |
| **プロセス階層** | ppidによる親子追跡 |

---

### スケジューラ

| パラメータ | 値 |
|:--|:--|
| **方式** | CFS、プリエンプティブ |
| **最大プロセス数** | 4096 |
| **タイマー周波数** | 250 Hz (PIT) |
| **CPU窓** | 250ティック (1秒) |
| **スタック** | プロセスあたり 512 KB |
| **状態** | Ready / Running / Sleeping / Blocked / Dead |
| **実装** | ロックフリー: ISRはアトミックのみ使用 |
| **優先度** | 1-20スケール、重み付きvruntime |
| **アフィニティ** | プロセスごとのCPUマスク |

---

### システムコール

| Nr | 名前 | 説明 |
|:--:|:--|:--|
| **0** | `sys_exit` | プロセス終了 + yield |
| **1** | `sys_write` | stdout/stderrへの書き込み (fd 1/2) |
| **2** | `sys_read` | stdin (fd 0) またはファイルディスクリプタからの読み込み |
| **3** | `sys_mmap` | メモリマッピングの作成 |
| **4** | `sys_munmap` | メモリマッピングの解除 |
| **5** | `sys_mprotect` | メモリ保護属性の変更 |
| **6** | `sys_brk` | ヒープの拡張 |
| **7** | `sys_getpid` | 現在のプロセスのPIDを取得 |
| **8** | `sys_getcwd` | カレントディレクトリの取得 |
| **9** | `sys_set_tls` | FS.baseレジスタの設定 (TLS) |
| **10** | `sys_get_tls` | FS.baseレジスタの取得 |
| **11** | `sys_open` | ファイルを開く (VFS + ext2) |
| **12** | `sys_close` | ファイルディスクリプタを閉じる |
| **13** | `sys_seek` | ファイルオフセットの設定 |
| **14** | `sys_fsize` | ファイルサイズの取得 |
| **15** | `sys_map_lib` | 共有ライブラリの直接マッピング |
| **16** | `sys_sleep` | プロセスをスリープ (~4ms/ティック) |
| **17** | `sys_uptime` | 起動からのティック数を取得 |
| **18** | `sys_stat` | ファイル情報の取得 |
| **19** | `sys_fstat` | FD経由のファイル情報取得 |
| **20** | `sys_mkdir` | ディレクトリの作成 |
| **21** | `sys_rmdir` | ディレクトリの削除 |
| **22** | `sys_unlink` | ファイルの削除 |
| **23** | `sys_readdir` | ディレクトリエントリの読み込み |
| **24** | `sys_rename` | ファイル/ディレクトリの改名 |
| **25** | `sys_link` | ハードリンクの作成 |
| **26** | `sys_chmod` | パーミッションの変更 |
| **27** | `sys_chown` | 所有者の変更 |
| **28** | `sys_dup` | ファイルディスクリプタの複製 |
| **29** | `sys_dup2` | 指定FDへの複製 |
| **30** | `sys_truncate` | ファイルの切り詰め |
| **31** | `sys_write_file` | ファイル内容の書き込み |
| **32** | `sys_symlink` | シンボリックリンクの作成 |
| **33** | `sys_readlink` | シンボリックリンクの読み込み |
| **34** | `sys_pipe` | パイプの作成 |
| **35** | `sys_chdir` | ディレクトリの変更 |
| **36** | `sys_statfs` | ファイルシステム統計 |
| **37** | `sys_fallocate` | ファイル空間の事前確保 |
| **38** | `sys_getxattr` | 拡張属性の取得 |
| **39** | `sys_setxattr` | 拡張属性の設定 |
| **40** | `sys_utimensat` | タイムスタンプの設定 |
| **41** | `sys_fsync` | ディスクへのフラッシュ |
| **42** | `sys_punch_hole` | ファイルのホールパンチ |
| **43** | `sys_fork` | プロセスのフォーク |
| **44** | `sys_wait4` | 子プロセスの待機 |
| **45** | `sys_kill` | シグナルの送信 |
| **46** | `sys_exec` | ELFバイナリの実行 |

FDテーブルはプロセスごとに管理 (BTreeMap<pid, ProcessFds>)。

---

### ネットワークスタック

<details>
<summary><b>ネットワークカードドライバ</b></summary>

| ドライバ | チップ |
|:--|:--|
| **Intel E1000** | 82540EM、82545EM、82574L、82579LM、I217 |
| **Realtek RTL8139** | RTL8139 |
| **Realtek RTL8168** | RTL8168、RTL8169 |
| **VirtIO Net** | QEMU/KVM仮想ネットワークカード |

</details>

<details>
<summary><b>プロトコル</b></summary>

| レイヤー | プロトコル |
|:--|:--|
| **L2** | Ethernet、ARP (キャッシュテーブル付き) |
| **L3** | IPv4、ICMP |
| **L4** | UDP、TCP (コネクション状態管理付き) |
| **アプリケーション** | DHCP、DNS、NTP、HTTP、HTTP/2、Traceroute |
| **セキュリティ** | TLS 1.3 (ECDHE + RSA + AES-GCM) |

</details>

<details>
<summary><b>TLS 1.3: ゼロからの完全実装</b></summary>

- ECDH: X25519鍵交換 (`tls_ecdh.rs`)
- RSA: ASN.1/DER証明書のパース、PKCS#1署名検証 (`tls_rsa.rs`)
- BigNum: RSA 2048-bit用の独自大数演算実装 (`tls_bignum.rs`)
- AES-GCM: 認証付き対称暗号化 (`tls_gcm.rs`)
- SHA-256、HMAC、HKDF: ハッシュ化、鍵導出 (`tls_crypto.rs`)
- ハンドシェイク: ClientHello → ServerHello → Certificate → Finished

</details>

---

### VFS (仮想ファイルシステム)

<details>
<summary><b>展開する</b></summary>

#### 基本機能

| パラメータ | 値 |
|:--|:--|
| **VNode数** | 256 |
| **同時オープンファイル数** | 32 |
| **マウントポイント** | 8 |
| **子ノード数** | 動的 (上限なし) |

子ノードは動的 `Vec` ベースのハッシュマップで管理されます。初期スロット数は16で、使用率75%に達すると自動的に2倍に拡張されます。

- ノードタイプ: `Regular`、`Directory`、`Symlink`、`CharDevice`、`BlockDevice`、`Pipe`、`Fifo`、`Socket`
- 権限、uid/gid、タイムスタンプ、サイズ、nlinksの完全なメタデータ付き

#### システムライブラリ

ブート時に `/lib` ディレクトリをtmpfsに作成し、`libmiku.so` をimmutableファイルとして書き込みます。
immutableフラグにより unlink / write / rename は拒否されます。

#### キャッシュ

| キャッシュ | サイズ |
|:--|:--|
| **ページキャッシュ** | 128ページ x 512バイト、LRU追い出し |
| **Dentryキャッシュ** | 128エントリ、FNV32ハッシュ |

#### ナビゲーション

- パスウォーキング: 深さ最大32コンポーネント
- シンボリックリンク解決: ループ保護 (8レベル)
- FNV32ハッシュ: O(1)ルックアップのための名前ハッシュ化

#### セキュリティ

- UNIXパーミッションモデル: `owner/group/other`、`setuid/setgid/sticky`
- セキュリティラベル (MAC)、バイトとinode単位のクォータ
- ファイルロック: デッドロック検出付きshared/exclusive (最大16ロック)
- immutableフラグ: システムライブラリの保護

#### 高度な機能

| 機能 | 詳細 |
|:--|:--|
| **VFSジャーナル** | 16件の操作ログ |
| **Xattr** | ノードあたり8つの拡張属性 |
| **Notifyイベント** | inotify的サブシステム (最大16イベント) |
| **バージョンストア** | ファイルの16スナップショット |
| **CASストア** | コンテンツアドレス指定の重複排除 (最大16オブジェクト) |
| **ブロックI/Oキュー** | 8件の非同期リクエスト |

</details>

---

### ファイルシステム

| FS | マウントポイント | 説明 |
|:--:|:--:|:--|
| **tmpfs** | `/` | RAMベースのルートFS |
| **devfs** | `/dev` | デバイス: `null`、`zero`、`random`、`urandom`、`console` |
| **procfs** | `/proc` | `version`、`uptime`、`meminfo`、`mounts`、`cpuinfo`、`stat` |
| **ext2** | `/mnt` | 実ディスクへの完全な読み書き |
| **ext3** | `/mnt` | ext2上のジャーナリング (JBD2)、遅延書き込み |
| **ext4** | `/mnt` | エクステントベースファイル + crc32cチェックサム |

---

### MikuFS: Ext2/3/4ドライバ

<details>
<summary><b>展開する</b></summary>

#### 読み込み

- スーパーブロック、グループディスクリプタ、inode、ディレクトリエントリ
- 間接ブロック (シングル / ダブル / トリプル)
- Ext4エクステントツリー

#### 書き込み

- ファイル、ディレクトリ、シンボリックリンクの作成と削除
- ブロックとinode用ビットマップアロケータ (優先グループ対応)
- 再帰的な削除
- 遅延書き込み (dirty cache + pdflush)

#### Ext3ジャーナル (JBD2)

- ジャーナルの作成 (`ext2 → ext3` 変換)
- トランザクションの書き込み: ディスクリプタブロック、コミットブロック、revokeブロック
- リカバリ: マウント時に未完了トランザクションをリプレイ
- 遅延コミット: journal書き込みをdirty cacheで高速化

#### mkfs

- ext2/ext3/ext4のフォーマット対応
- lazy init: group 0のメタデータのみ即時初期化、残りは遅延
- ジャーナルスーパーブロックのみ初期化 (全ブロックの零化を省略)

#### ユーティリティ

- `fsck`、`tree`、`du`、`cp`、`mv`、`chmod`、`chown`、ハードリンク

</details>

---

### シェルコマンド

#### サービス管理 (sv)

| コマンド | 説明 |
|:--|:--|
| `sv list` | 全サービスの一覧 |
| `sv status <name>` | 詳細なステータス |
| `sv start/stop/restart <name>` | サービスライフサイクル |
| `sv reload <name>` | SIGHUP送信 |
| `sv enable/disable <name>` | 有効化/無効化 |
| `sv mask/unmask <name>` | マスク/アンマスク |
| `sv force-stop <name>` | 強制終了 |
| `sv journal [name]` | イベントログ |
| `sv target [name]` | ターゲット管理 |
| `sv analyze` | ブート分析 |
| `sv tree/rdeps <name>` | 依存関係情報 |
| `sv load/scan` | ユニットファイル管理 |
| `sv timer list/start/stop` | タイマー管理 |

#### 統合extコマンド (マウントされたFSバージョンを自動検出)

| コマンド | 構文 | 説明 |
|:--|:--|:--|
| `ext2mount` | `ext2mount [drive]` | ext2マウント |
| `ext3mount` | `ext3mount [drive]` | ext3マウント |
| `ext4mount` | `ext4mount [drive]` | ext4マウント |
| `extls` | `extls [path]` | ディレクトリ一覧 |
| `extcat` | `extcat <path>` | ファイル内容表示 |
| `extstat` | `extstat <path>` | inodeの詳細 |
| `extinfo` | `extinfo` | スーパーブロック情報 |
| `extwrite` | `extwrite <path> <text>` | ファイルへの書き込み |
| `extappend` | `extappend <path> <text>` | ファイルへの追記 |
| `exttouch` | `exttouch <path>` | 空ファイルの作成 |
| `extmkdir` | `extmkdir <path>` | ディレクトリの作成 |
| `extrm` | `extrm [-rf] <path>` | ファイルの削除 |
| `extrmdir` | `extrmdir <path>` | 空ディレクトリの削除 |
| `extmv` | `extmv <path> <newname>` | ファイルの改名 |
| `extcp` | `extcp <src> <dst>` | ファイルのコピー |
| `extln -s` | `extln -s <target> <link>` | シンボリックリンクの作成 |
| `extlink` | `extlink <existing> <link>` | ハードリンクの作成 |
| `extchmod` | `extchmod <mode> <path>` | パーミッションの変更 |
| `extchown` | `extchown <uid> <gid> <path>` | 所有者の変更 |
| `extdu` | `extdu [path]` | ディスク使用量 |
| `exttree` | `exttree [path]` | ディレクトリツリー |
| `extfsck` | `extfsck` | FSの整合性チェック |
| `extcache` | `extcache` | ブロックキャッシュ統計 |
| `extcacheflush` | `extcacheflush` | キャッシュのフラッシュ |
| `extsync` / `sync` | `sync` | ディスクへの書き込み |

> 旧コマンド (`ext2ls`、`ext3cat`、`ext4write` 等) は後方互換性のために残っています。

#### VFSコマンド

| コマンド | 説明 |
|:--|:--|
| `ls [path]` | ディレクトリ一覧 (ext + VFS統合表示) |
| `cd <path>` | ディレクトリ移動 |
| `pwd` | 現在のパス表示 |
| `mkdir <path>` | ディレクトリ作成 |
| `touch <path>` | ファイル作成 (RAM) |
| `cat <path>` | ファイル内容表示 |
| `write <path> <text>` | ファイルへの書き込み (RAM) |
| `rm [-rf] <path>` | ファイル/ディレクトリ削除 |
| `rmdir <path>` | ディレクトリ削除 (ext対応) |
| `mv <old> <new>` | 改名 |
| `stat <path>` | ファイル情報 |
| `chmod <mode> <path>` | パーミッション変更 |
| `df` | ファイルシステム情報 |

#### ダイナミックリンクコマンド

| コマンド | 説明 |
|:--|:--|
| `exec <path>` | ELFバイナリの実行 (ダイナミックリンク対応) |
| `ldconfig` | 共有ライブラリキャッシュの更新 |
| `ldd` | キャッシュされたライブラリの一覧表示 |

#### プロセス管理コマンド

| コマンド | 説明 |
|:--|:--|
| `ps` | 全プロセスの一覧 |
| `top` | プロセスモニター |
| `kill <pid>` | PIDでプロセスを終了 |
| `nice <pid> <prio>` | プロセスの優先度変更 |
| `affinity <pid> <mask>` | CPUアフィニティの設定 |

#### システムコマンド

| コマンド | 説明 |
|:--|:--|
| `poweroff` / `shutdown` | mikuD経由のグレースフルシャットダウン |
| `reboot` | mikuD経由のグレースフルリブート |
| `info` | システム情報 |
| `memmap` | メモリマップ |
| `heap` | ヒープ統計 |
| `clear` | 画面クリア |
| `echo <text>` | テキスト出力 |
| `history` | コマンド履歴 |
| `help` | コマンド一覧 |

#### mkfsコマンド

| コマンド | 説明 |
|:--|:--|
| `mkfs.ext2 <drive>` | ext2フォーマット |
| `mkfs.ext3 <drive>` | ext3フォーマット (ジャーナル付き) |
| `mkfs.ext4 <drive>` | ext4フォーマット (エクステント + ジャーナル) |

---

### ATAドライバ

| パラメータ | 値 |
|:--|:--|
| **モード** | PIO (プログラムI/O) |
| **操作** | セクターの読み書き (512バイト)、最大255セクター/コマンド |
| **ディスク数** | 4台: Primary/Secondary x Master/Slave |
| **保護** | 書き込み後のキャッシュフラッシュ、タイムアウト 50Kイテレーション |
| **アドレス指定** | LBA28 (最大128GB) |

---

## ビルドと実行

### 必要なツール

| ツール | 用途 |
|:--|:--|
| **Rust nightly** | `no_std` + コンパイラの不安定な機能 |
| **QEMU** | x86_64マシンのエミュレーション |
| **grub-mkrescue** | ブータブルISOの作成 |
| **GCC** | libmiku stub生成 + Cプログラムのコンパイル |
| **e2tools** | ext4イメージへのファイルコピー |
| **Cargo** | カーネルのビルド |

### 実行手順

```bash
git clone https://github.com/altushkaso2/miku-os
cd miku-os/builder
cargo run
```

Builderがすべて自動で行います:

```
RAMの節約モード? (y/N)
[1/7] ld-miku.soのコンパイル
[2/7] libmiku.soのコンパイル
[3/7] miku-osカーネルのコンパイル
[4/7] ファイル構造の作成
[5/7] システムイメージの生成 (miku-os.iso)
[6/7] ディスクの準備
[7/7] QEMUの起動 (任意 (y/N))
```

### userspace プログラムのビルド

```bash
cd src/lib/userspace
./build.sh hello         # ビルド + ディスクにコピー
./build.sh test_full     # テストスイート
./build.sh               # 全バイナリ
```

---

## MikuOS ABI

userspace プログラムの開発に関する完全なドキュメントは [MikuOS_ABI.md](docs/MikuOS_ABI.md) を参照してください。

---

## 作者

<div align="center">
  <a href="https://github.com/alunwrd">
    <img src="https://github.com/altushkaso2.png" width="100" style="border-radius:50%;" alt="alunwrd">
  </a>
  <br><br>
  <a href="https://github.com/alunwrd"><b>@alunwrd</b></a>
  <br>
  <sub>Miku OSの作者および唯一の開発者</sub>
  <br>
  <sub>カーネル - VFS - MikuFS - ELF - ld-miku - libmiku - シェル - ネットワーク - TLS - スケジューラ - PMM - VMM - Swap - mikuD - シグナル - fork/exec</sub>
</div>

---

## 作者より

> すべては「自分でOSを書いてみたらどうなるだろう?」というシンプルな思いから始まりました。
> 毎晩、新しい機能を追加し、新しいバグを直し、新しい発見をしています。
> 画面への最初の文字表示から、本格的なTLS 1.3スタック、ロックフリースケジューラ、
> そしてダイナミックリンカーまで、すべて手作業で書きました。
> 既製のライブラリやラッパーは一切なし。Rustとドキュメントと根気だけです :D
>
> ELFローダーとダイナミックリンクが動いた瞬間、「hello from dynamic linking!」が
> 画面に表示された時の感動は忘れられません。
> libmikuが1617テスト全て通った瞬間、このOSで本当のプログラムが動くことを実感しました。

<div align="center">

**Miku OS** - Rustでゼロから書かれた純粋なOS

*愛を込めて*

<img src="https://raw.githubusercontent.com/alunwrd/miku-os/main/docs/miku.png" width="220" alt="Miku Logo">
