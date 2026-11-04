use mikupkg::format::db::Installed;
use mikupkg::format::meta as meta_codec;

#[test]
fn the_database_the_build_wrote_reads_back() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/fw_root/pkg/db");
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => { eprintln!("no build output; skipping"); return; }
    };
    let mut seen = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().map(|x| x != "mpkr").unwrap_or(true) { continue; }
        let raw = std::fs::read(&p).unwrap();
        let rec = Installed::parse(&raw).unwrap_or_else(|e| panic!("{p:?}: {e}"));
        let meta = meta_codec::decode(&rec.meta, |t, a| panic!("byte {a}: tag {t:#06x}")).unwrap();
        println!("{} {}  {} files  {}  \"{}\"",
            meta.name, meta.version, rec.files.entries.len(),
            if rec.reason == mikupkg::format::db::Reason::Explicit { "explicit" } else { "dependency" },
            meta.desc);
        for d in &meta.depends { println!("      depends {d}"); }
        seen.push(meta.name.to_string());
    }
    // `zed`, `miku-firmware` and `miku-personal` depend on what the build found
    for name in ["miku-base", "miku-system", "miku-desktop", "miku-net", "miku-demos"] {
        assert!(seen.iter().any(|s| s == name), "{name} is not in the database: {seen:?}");
    }
}
