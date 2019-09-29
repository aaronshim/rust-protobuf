//! Common code of `build.rs` of two tests

pub use protobuf_codegen::Customize;

use std::fmt;
use std::fs;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Read;
use std::io::Write;
use std::path::Path;

use glob;

pub fn glob_simple(pattern: &str) -> Vec<String> {
    let mut r: Vec<_> = glob::glob(pattern)
        .expect("glob")
        .map(|g| {
            g.expect("item")
                .as_path()
                .to_str()
                .expect("utf-8")
                .to_owned()
        })
        .collect();
    // Make test stable
    r.sort();
    r
}

fn read_gitignore(dir: &Path) -> Vec<String> {
    let mut patterns = Vec::new();

    let gitignore = format!("{}/.gitignore", dir.display());
    let gitignore = &Path::new(&gitignore);
    if gitignore.exists() {
        let gitignore = fs::File::open(gitignore).expect("open gitignore");
        for line in BufReader::new(gitignore).lines() {
            let line = line.expect("read_line");
            if line.is_empty() || line.starts_with("#") {
                continue;
            }
            patterns.push(line);
        }
    }

    patterns
}

fn clean_recursively(dir: &Path, patterns: &[&str]) {
    assert!(dir.is_dir());

    let gitignore_patterns = read_gitignore(dir);

    let mut patterns = patterns.to_vec();
    patterns.extend(gitignore_patterns.iter().map(String::as_str));

    let patterns_compiled: Vec<_> = patterns
        .iter()
        .map(|&p| glob::Pattern::new(p).expect("failed to compile pattern"))
        .collect();

    for entry in fs::read_dir(dir).expect("read_dir") {
        let entry = entry.expect("entry");
        let entry_path = entry.path();
        let file_name = entry_path.as_path().file_name().unwrap().to_str().unwrap();
        if entry.metadata().expect("metadata").is_dir() {
            clean_recursively(&entry_path, &patterns);
        } else if file_name == ".gitignore" {
            // keep it
        } else {
            for pattern in &patterns_compiled {
                if pattern.matches(file_name) {
                    fs::remove_file(&entry_path).expect("remove_file");
                    break;
                }
            }
        }
    }
}

pub fn clean_old_files() {
    clean_recursively(&Path::new("src"), &["*_pb.rs", "*_pb_proto3.rs"]);
}

#[derive(Default)]
pub struct GenInDirArgs<'a> {
    pub out_dir: &'a str,
    pub input: &'a [&'a str],
    pub includes: &'a [&'a str],
    pub customize: Customize,
}

/// Generate mod.rs from all files in a directory
pub fn gen_mod_rs_in_dir(dir: &str) {
    assert!(Path::new(dir).is_dir());

    let mut mod_rs = fs::File::create(&format!("{}/mod.rs", dir)).expect("create");

    writeln!(mod_rs, "// generated by {}", module_path!()).expect("write");
    writeln!(mod_rs, "").expect("write");

    let rs_files = glob_simple(&format!("{}/*.rs", dir));

    for rs in rs_files {
        let file_name = Path::new(&rs)
            .file_name()
            .expect("file_name")
            .to_str()
            .expect("file_name");
        if file_name == "mod.rs" {
            continue;
        }
        assert!(file_name.ends_with(".rs"));
        let mod_name = &file_name[..file_name.len() - ".rs".len()];

        if mod_name.contains("carllerche") {
            writeln!(mod_rs, r#"#[cfg(feature = "with-bytes")]"#).expect("write");
        }
        if mod_name.contains("test_map_simple") {
            writeln!(mod_rs, r#"#[cfg(protoc3)]"#).expect("write");
        }
        writeln!(mod_rs, "mod {};", mod_name).expect("write");
    }

    mod_rs.flush().expect("flush");
}

pub fn gen_in_dir_impl<F, E>(dir: &str, include_dir: &str, protoc3: bool, gen: F)
where
    F: for<'a> Fn(GenInDirArgs<'a>) -> Result<(), E>,
    E: fmt::Debug,
{
    info!("generating protos in {}", dir);

    let mut protos = Vec::new();
    for suffix in &[".proto", ".proto3"] {
        protos.extend(glob_simple(&format!("{}/*{}", dir, suffix)));
    }

    if !protoc3 {
        // Remove test_map_simple because map is not supported by protoc 2
        protos.retain(|p| !p.contains("test_map_simple"));
    }

    assert!(!protos.is_empty());

    gen(GenInDirArgs {
        out_dir: dir,
        input: &protos.iter().map(|a| a.as_ref()).collect::<Vec<&str>>(),
        includes: &["../proto", include_dir],
        ..Default::default()
    })
    .expect("protoc");

    gen_mod_rs_in_dir(dir);
}

pub fn list_tests_in_dir(dir: &str) -> Vec<String> {
    let mut r = Vec::new();
    for entry in fs::read_dir(dir).expect("read_dir") {
        let entry = entry.expect("entry");
        let entry_path = entry.path();
        let file_name = entry_path.as_path().file_name().unwrap().to_str().unwrap();

        // temporart files
        if file_name.ends_with(".") {
            continue;
        }

        if !file_name.ends_with(".rs") || file_name.ends_with("_pb.rs") {
            continue;
        }

        if file_name == "mod.rs" {
            continue;
        }

        r.push(file_name[..file_name.len() - ".rs".len()].to_owned());
    }

    // Make test stable
    r.sort();

    r
}

pub fn copy_tests_v2_v3(v2_dir: &str, v3_dir: &str) {
    for test_name in list_tests_in_dir(v2_dir) {
        let mut p2f =
            fs::File::open(&format!("{}/{}_pb.proto", v2_dir, test_name)).expect("open v2 .proto");
        let mut proto = String::new();
        p2f.read_to_string(&mut proto).expect("read .proto");
        drop(p2f);

        let mut r2f = fs::File::open(&format!("{}/{}.rs", v2_dir, test_name)).expect("open v2 .rs");
        let mut rs = String::new();
        r2f.read_to_string(&mut rs).expect("read .rs");
        drop(r2f);

        let mut p3f = fs::File::create(&format!("{}/{}_pb.proto", v3_dir, test_name))
            .expect("create v3 .proto");
        let mut r3f =
            fs::File::create(&format!("{}/{}.rs", v3_dir, test_name)).expect("create v3 .rs");

        // convert proto2 to proto3
        let proto = proto.replace("optional ", "");
        let proto = proto.replace("required ", "");
        let proto = proto.replace("syntax = \"proto2\";", "syntax = \"proto3\";");
        write!(p3f, "// generated\n").expect("write");
        write!(p3f, "{}", proto).expect("write");
        p3f.flush().expect("flush");

        write!(r3f, "// generated\n").expect("write");
        write!(r3f, "{}", rs).expect("write");
        r3f.flush().expect("flush");
    }
}
