use crate::{
    proto_file_builder::ProtoFileBuilder,
    tree::{Forest, NamespaceChild},
}; // Assuming your tree.rs is in the same crate
use itertools::Itertools;
use tonic_build::Config;
use std::{
    collections::{HashMap, HashSet},
    io,
    path::{Path, PathBuf},
};

macro_rules! io_error {
    ($e:expr) => {
        io::Error::new(io::ErrorKind::Other, $e)
    };
}

macro_rules! ok_or_invalid_service {
    ($x:expr) => {
        $x.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid service definition"))
    };
}

#[derive(Debug)]
struct Endpoint<'a> {
    name: &'a str,
    req: &'a str,
    resp: &'a str,
    streaming: Option<&'a str>,
}

#[derive(Debug)]
struct Service<'a> {
    namespace: &'a str,
    name: &'a str,
    endpoints: Vec<Endpoint<'a>>,
}

impl<'a> Service<'a> {
    pub fn parse(buf: &'a str, namespace: &'a str) -> Result<Self, io::Error> {
        let mut service = Service {
            namespace,
            name: "",
            endpoints: Vec::new(),
        };

        let (_, buf) = ok_or_invalid_service!(buf.split_once(' '))?;

        let buf = service.read_name(buf);

        let mut buf = service.consume_char(buf, '{');

        while buf != "}" {
            let end_idx = ok_or_invalid_service!(buf.find(';'))?;
            service.parse_endpoint(&buf[..=end_idx])?;
            buf = buf[end_idx + 1..].trim();
        }

        Ok(service)
    }

    fn read_name(&mut self, buf: &'a str) -> &'a str {
        let Some(idx) = buf.find('{') else {
            return buf;
        };

        let name = buf[..idx].trim();
        let buf = buf[idx..].trim();

        self.name = name;
        buf
    }

    fn consume_char(&self, buf: &'a str, pat: char) -> &'a str {
        if let Some(idx) = buf.find(pat) {
            buf[idx + 1..].trim()
        } else {
            buf
        }
    }

    fn parse_endpoint(&mut self, buf: &'a str) -> Result<&'a str, io::Error> {
        let Some((input, output)) = buf.split_once(':') else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid endpoint syntax",
            ));
        };

        let input = input.trim();
        let output = output.trim();

        let Some(start_idx) = input.find('(') else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid endpoint syntax",
            ));
        };

        let Some(end_idx) = input.find(')') else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid endpoint syntax",
            ));
        };

        let endpoint_name = &input[..start_idx];
        let input_ident = input[start_idx + 1..end_idx].trim();

        let (buf, output_name, meta) = if let Some(paren_idx) = output.find('(') {
            let name = output[..paren_idx].trim();
            let Some(end_paren_idx) = output.find(')') else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid endpoint syntax",
                ));
            };

            let Some(end_idx) = output.find(';') else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid endpoint syntax",
                ));
            };

            (
                output[end_idx + 1..].trim(),
                name,
                Some(output[paren_idx + 1..end_paren_idx].trim()),
            )
        } else if let Some(end_idx) = output.find(';') {
            let name = output[..end_idx].trim();
            (output[end_idx + 1..].trim(), name, None)
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid endpoint syntax",
            ));
        };

        let meta = meta.map(|meta| {
            let Some((ident, code)) = meta.split_once(':') else {
                return "none";
            };

            let ident = ident.trim();
            let code = code.trim();

            if ident != "streaming" {
                return "none";
            }

            let code = code[1..code.len() - 1].trim();

            code
        });

        self.endpoints.push(Endpoint {
            name: endpoint_name,
            req: input_ident,
            resp: output_name,
            streaming: meta,
        });

        Ok(buf.trim())
    }
}

pub fn configure() -> Builder {
    Builder {
        build_client: true,
        build_server: true,
        emit_rerun_if_changed: true,
        generate_default_stubs: false,
        out_dir: None,
    }
}

pub struct Builder {
    pub(crate) build_client: bool,
    pub(crate) build_server: bool,
    pub(crate) emit_rerun_if_changed: bool,
    pub(crate) generate_default_stubs: bool,

    out_dir: Option<PathBuf>,
}

impl Builder {
    /// Enable or disable gRPC client code generation.
    pub fn build_client(mut self, enable: bool) -> Self {
        self.build_client = enable;
        self
    }

    /// Enable or disable gRPC server code generation.
    pub fn build_server(mut self, enable: bool) -> Self {
        self.build_server = enable;
        self
    }

    /// Set the output directory to generate code to.
    ///
    /// Defaults to the `OUT_DIR` environment variable.
    pub fn out_dir(mut self, out_dir: impl AsRef<Path>) -> Self {
        self.out_dir = Some(out_dir.as_ref().to_path_buf());
        self
    }

    /// Enable or disable emitting
    /// [`cargo:rerun-if-changed=PATH`](https://doc.rust-lang.org/cargo/reference/build-scripts.html#rerun-if-changed)
    /// instructions for Cargo.
    ///
    /// If set, writes instructions to `stdout` for Cargo so that it understands
    /// when to rerun the build script. By default, this setting is enabled if
    /// the `CARGO` environment variable is set. The `CARGO` environment
    /// variable is set by Cargo for build scripts. Therefore, this setting
    /// should be enabled automatically when run from a build script. However,
    /// the method of detection is not completely reliable since the `CARGO`
    /// environment variable can have been set by anything else. If writing the
    /// instructions to `stdout` is undesirable, you can disable this setting
    /// explicitly.
    pub fn emit_rerun_if_changed(mut self, enable: bool) -> Self {
        self.emit_rerun_if_changed = enable;
        self
    }

    /// Enable or disable directing service generation to providing a default implementation for service methods.
    /// When this is false all gRPC methods must be explicitly implemented.
    /// When this is true any unimplemented service methods will return 'unimplemented' gRPC error code.
    /// When this is true all streaming server request RPC types explicitly use tonic::codegen::BoxStream type.
    ///
    /// This defaults to `false`.
    pub fn generate_default_stubs(mut self, enable: bool) -> Self {
        self.generate_default_stubs = enable;
        self
    }

    /// Compile the .proto files and execute code generation.
    pub fn compile(self, fbs: &[impl AsRef<Path>]) -> io::Result<()> {
        let mut flatbuffers_opts = flatbuffers_build::BuilderOptions::new_with_files(fbs);

        // Set options
        if !self.emit_rerun_if_changed {
            flatbuffers_opts = flatbuffers_opts.supress_buildrs_directives();
        }

        if let Some(path) = self.out_dir.as_ref() {
            flatbuffers_opts = flatbuffers_opts.set_output_path(path);
        }

        let out_dir = self
            .out_dir
            .as_ref()
            .map(|path| {
                path.to_str().map(ToOwned::to_owned).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::Other, "out_dir path is not valid unicode")
                })
            })
            .unwrap_or_else(|| {
                std::env::var("OUT_DIR").map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::Other,
                        "OUT_DIR environment variable is not set",
                    )
                })
            })?;

        // Compile flatbuffers
        flatbuffers_opts
            .compile()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        println!("Output directory: {out_dir}");

        // --- Build the Forest from all .fbs files ---
        let forest = Forest::from_fbs_files(fbs)?;

        // --- Extract protos from the Forest ---
        ProtoFileBuilder::new(&forest, &out_dir)
            .build_all()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        Self::run_tonic_build(&out_dir, self.generate_default_stubs)?;
        Self::cleanup_proto_files(&out_dir)?;

        // Self::postprocess_generated_code(out_dir, file_namespace, fake_types);
        Ok(())
    }

    fn extract_service_bufs(file: &str) -> Vec<(usize, &str)> {
        let mut service_bufs = Vec::new();
        let mut idx = 0;
        while idx < file.len() {
            if let Some(match_idx) = file[idx..].find("rpc_service") {
                let match_idx = idx + match_idx;
                let Some(end_idx) = file[match_idx..].find('}') else {
                    break;
                };
                let end_idx = match_idx + end_idx;
                service_bufs.push((match_idx, file[match_idx..=end_idx].trim()));
                idx = match_idx + 1;
            } else {
                break;
            }
        }
        service_bufs
    }

    fn generate_fake_proto_files(forest: &Forest) -> HashMap<String, String> {
        // --- Traverse the Forest for code generation ---
        for node in forest.iter() {
            match node {
                NamespaceChild::Namespace(ns_rc) => {
                    // Generate mod.rs, recurse, etc.
                    println!("Generating module for namespace: {}", ns_rc.path());
                    // You can call your codegen helpers here, passing ns_rc
                }
                NamespaceChild::Service(svc_rc) => {
                    // Generate service code
                    println!("Generating service: {}", svc_rc.path());
                }
                NamespaceChild::Type(ty_rc) => {
                    // Generate type code or macro
                    println!("Generating type: {}", ty_rc.path());
                }
            }
        }

        let mut files = HashMap::new();
        // for (namespace, types) in fake_types {
        //     // No leading/trailing whitespace before/after package line
        //     let mut file_str = format!("syntax = \"proto3\";\npackage {};\n", namespace.trim());
        //     for typename in types {
        //         let (_, typename) = typename.rsplit_once('.').unwrap_or(("", typename));
        //         file_str.push_str(&format!("\nmessage {} {{}}", typename));
        //     }
        //     let filename = format!("fake_{}.proto", namespace.trim());
        //     println!("DEBUG: Generating proto file: {}", filename);
        //     println!("DEBUG: Proto file content:\n{}", file_str);
        //     files.insert(filename, file_str);
        // }
        files
    }

    fn generate_main_proto(
        file_namespace: &str,
        service_proto: &str,
        referenced_namespaces: &HashSet<String>,
    ) -> String {
        let file_includes = referenced_namespaces
            .iter()
            .filter(|ns| !ns.is_empty())
            .map(|ns| format!("import \"fake_{}.proto\";", ns.trim()))
            .collect::<Vec<_>>()
            .join("\n");

        let main_proto = format!(
            "syntax = \"proto3\";\n{file_includes}\npackage {};\n\n{service_proto}",
            file_namespace.trim()
        );

        println!("DEBUG: Main proto file content:\n{}", main_proto);

        main_proto
    }

    fn run_tonic_build(out_dir: &str, generate_default_stubs: bool) -> io::Result<()> {
        let mut config = Config::new();
        config.protoc_executable(protobuf_src::protoc());
        tonic_build::configure()
            .build_client(true)
            .build_server(true)
            .codec_path("::tonic_flatbuffers::codec::FlatCodec")
            .emit_rerun_if_changed(false)
            .generate_default_stubs(generate_default_stubs)
            .out_dir(out_dir)
            .compile_protos_with_config(config, &[&format!("{out_dir}/generated.proto")], &[out_dir])?;
        Ok(())
    }

    fn cleanup_proto_files(out_dir: &str) -> io::Result<()> {
        for path in std::fs::read_dir(out_dir)?.flatten() {
            if let Ok(file_type) = path.file_type() {
                let Ok(file_name) = path.file_name().into_string() else {
                    continue;
                };
                if file_type.is_file() && file_name.ends_with(".proto") {
                    let _ = std::fs::remove_file(path.path());
                }
            }
        }
        Ok(())
    }

    fn postprocess_generated_code(
        out_dir: &str,
        file_namespace: &str,
        fake_types: &HashMap<String, HashSet<String>>,
    ) -> io::Result<()> {
        let rust_file_path = format!("{out_dir}/{file_namespace}.rs");
        println!(
            "[postprocess] Looking for generated Rust file: {}",
            rust_file_path
        );

        if !std::path::Path::new(&rust_file_path).exists() {
            eprintln!(
                "[postprocess] Expected generated Rust file does not exist: {}",
                rust_file_path
            );
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("Generated Rust file not found: {}", rust_file_path),
            ));
        }

        let mut file = match std::fs::read_to_string(&rust_file_path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!(
                    "[postprocess] Failed to read generated Rust file '{}': {}",
                    rust_file_path, e
                );
                return Err(e);
            }
        };
        println!(
            "[postprocess] Read generated Rust file ({} bytes)",
            file.len()
        );
        println!("[postprocess] Rust file BEFORE modification:\n{}", file);

        let mut owned_list = HashSet::new();

        // Remove generated fake types
        if let Some(type_list) = fake_types.get(file_namespace) {
            for fake_type in type_list {
                let (_, typename) = fake_type.rsplit_once('.').unwrap_or(("", fake_type));
                owned_list.insert(typename);
                let pat = match regex::Regex::new(&format!(
                    r"(?:#\[\w+\([\w, =:]+\)\]\n)*pub struct (?:{typename}) \{{\}}\n"
                )) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!(
                            "[postprocess] Regex error for typename '{}': {}",
                            typename, e
                        );
                        return Err(io::Error::new(io::ErrorKind::Other, e));
                    }
                };

                let before = file.len();
                file = pat.replace(&file, "").into_owned();
                let after = file.len();
                if before != after {
                    println!(
                        "[postprocess] Removed fake type '{}' ({} bytes removed)",
                        typename,
                        before - after
                    );
                }
            }
        }

        let owned_list = owned_list
            .iter()
            .map(|name| name.strip_prefix("Owned").unwrap())
            .join(",");

        let header = "use super::*;";
        file = format!("{header}\n{file}");

        let mut namespace_path = PathBuf::new();
        namespace_path.push(out_dir);
        namespace_path.extend(file_namespace.split('.'));
        namespace_path.push("grpc.rs");

        println!("[postprocess] Rust file AFTER modification:\n{}", file);
        println!(
            "[postprocess] Writing processed Rust file to: {}",
            namespace_path.display()
        );
        if let Err(e) = std::fs::write(&namespace_path, &file) {
            eprintln!(
                "[postprocess] Failed to write processed Rust file '{}': {}",
                namespace_path.display(),
                e
            );
            return Err(e);
        }

        println!(
            "[postprocess] Removing original Rust file: {}",
            rust_file_path
        );
        if let Err(e) = std::fs::remove_file(&rust_file_path) {
            eprintln!(
                "[postprocess] Failed to remove original Rust file '{}': {}",
                rust_file_path, e
            );
            // Not fatal, so don't return here
        }

        let mod_rs_path = format!("{out_dir}/mod.rs");
        println!("[postprocess] Reading mod.rs: {}", mod_rs_path);
        let file = match std::fs::read_to_string(&mod_rs_path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!(
                    "[postprocess] Failed to read mod.rs '{}': {}",
                    mod_rs_path, e
                );
                return Err(e);
            }
        };
        println!("[postprocess] mod.rs BEFORE modification:\n{}", file);

        let mut mod_def: syn::ItemMod = match syn::parse_str(&file) {
            Ok(m) => m,
            Err(e) => {
                eprintln!(
                    "[postprocess] Failed to parse mod.rs as syn::ItemMod: {}",
                    e
                );
                return Err(io_error!(e));
            }
        };

        let namespace_segments = file_namespace.split('.').collect_vec();
        println!("[postprocess] Namespace segments: {:?}", namespace_segments);

        if namespace_segments.len() > 1 {
            if let Err(e) =
                Self::insert_mods_static(&namespace_segments, 0, &mut mod_def, &owned_list)
            {
                eprintln!(
                    "[postprocess] Failed to insert mods for namespace {:?}: {}",
                    namespace_segments, e
                );
                return Err(e);
            }
        } else if let Some((_, items)) = mod_def.content.as_mut() {
            if let Err(e) = Self::add_mods_static(items, &owned_list) {
                eprintln!("[postprocess] Failed to add mods: {}", e);
                return Err(e);
            }
        }

        let new_file = syn::File {
            attrs: vec![],
            items: vec![syn::Item::Mod(mod_def)],
            shebang: None,
        };

        let new_file = prettyplease::unparse(&new_file);

        println!("[postprocess] mod.rs AFTER modification:\n{}", new_file);
        println!("[postprocess] Writing updated mod.rs: {}", mod_rs_path);
        if let Err(e) = std::fs::write(&mod_rs_path, new_file) {
            eprintln!(
                "[postprocess] Failed to write updated mod.rs '{}': {}",
                mod_rs_path, e
            );
            return Err(e);
        }

        Ok(())
    }

    // Static versions for use in postprocess_generated_code
    fn insert_mods_static(
        segments: &[&str],
        cur_idx: usize,
        item: &mut syn::ItemMod,
        owned_list: &str,
    ) -> Result<(), io::Error> {
        if cur_idx >= segments.len() {
            return Ok(());
        }
        let cur_segment = segments[cur_idx];

        if item.ident == cur_segment && cur_idx == segments.len() - 1 {
            if let Some((_, items)) = item.content.as_mut() {
                Self::add_mods_static(items, owned_list)?;
            }
            Ok(())
        } else if item.ident == cur_segment {
            if let Some((_, items)) = item.content.as_mut() {
                for item in items {
                    if let syn::Item::Mod(item) = item {
                        Self::insert_mods_static(segments, cur_idx + 1, item, owned_list)?;
                    }
                }
            }
            Ok(())
        } else {
            Ok(())
        }
    }

    fn add_mods_static(items: &mut Vec<syn::Item>, owned_list: &str) -> Result<(), io::Error> {
        let item_mod: syn::ItemMod = syn::parse_str("pub mod grpc;").unwrap();
        let item = syn::Item::Mod(item_mod);
        items.push(item);

        let item_use: syn::ItemUse = syn::parse_str("pub use grpc::*;").unwrap();
        let item = syn::Item::Use(item_use);
        items.push(item);

        if !owned_list.is_empty() {
            let fb_owned_use: syn::ItemUse =
                syn::parse_str("use ::tonic_flatbuffers::flatbuffers_owned::*;").unwrap();
            let item = syn::Item::Use(fb_owned_use);
            items.push(item);

            let macro_def: syn::ItemMacro =
                syn::parse_str(&format!("flatbuffers_owned!({owned_list});"))
                    .map_err(|e| io_error!(e))?;
            let item = syn::Item::Macro(macro_def);
            items.push(item);
        }

        Ok(())
    }

    fn write_proto_files(out_dir: &str, extra_files: &HashMap<String, String>) -> io::Result<()> {
        for (filename, content) in extra_files {
            std::fs::write(format!("{}/{}", out_dir, filename), content)?;
        }
        Ok(())
    }

    fn inject_flatbuffers_owned_macros_per_namespace(
        out_dir: &str,
        defined_types: &HashMap<String, HashSet<String>>,
    ) -> std::io::Result<()> {
        for (namespace, types) in defined_types {
            if namespace.is_empty() || types.is_empty() {
                continue;
            }
            let mut rust_file_path = std::path::PathBuf::from(out_dir);
            for segment in namespace.split('.') {
                rust_file_path.push(segment);
            }
            rust_file_path.push("mod.rs");
            if !rust_file_path.exists() {
                rust_file_path.pop();
                rust_file_path.set_extension("rs");
                if !rust_file_path.exists() {
                    continue;
                }
            }
            println!("[inject] Processing file: {}", rust_file_path.display());
            let mut file = std::fs::read_to_string(&rust_file_path)?;
            println!("[inject] File BEFORE injection:\n{}", file);

            // Remove any previous macro block
            let macro_start = file.find("use ::tonic_flatbuffers::flatbuffers_owned::*;");
            let macro_end = file.find("flatbuffers_owned!(");
            if let (Some(start), Some(end)) = (macro_start, macro_end) {
                let macro_end = file[end..].find(';').map(|i| end + i + 1).unwrap_or(end);
                println!(
                    "[inject] Removing previous macro block (bytes {}..{})",
                    start, macro_end
                );
                file.replace_range(start..macro_end, "");
            }

            // Find where to inject: after the last "pub use"
            let insert_pos = file
                .rmatch_indices("pub use")
                .next()
                .map(|(idx, _)| file[idx..].find(';').map(|semi| idx + semi + 1))
                .flatten()
                .unwrap_or(0);

            let owned_list = types
                .iter()
                .map(|name| name.as_str())
                .collect::<Vec<_>>()
                .join(",");
            let macro_code = format!(
                "\nuse ::tonic_flatbuffers::flatbuffers_owned::*;\nflatbuffers_owned!({});\n",
                owned_list
            );

            println!(
                "[inject] Inserting macro at byte position {}:\n{}",
                insert_pos, macro_code
            );

            let (head, tail) = file.split_at(insert_pos);
            let new_file = format!("{}{}\n{}", head, macro_code, tail);

            println!("[inject] File AFTER injection:\n{}", new_file);

            std::fs::write(&rust_file_path, new_file)?;
            println!(
                "[inject] Injected flatbuffers_owned! macro for [{}] into {}",
                owned_list,
                rust_file_path.display()
            );
        }
        Ok(())
    }

    fn collect_defined_types(
        fbs_files: &[impl AsRef<Path>],
    ) -> io::Result<HashMap<String, HashSet<String>>> {
        let mut defined_types: HashMap<String, HashSet<String>> = HashMap::new();
        for file in fbs_files {
            let content = std::fs::read_to_string(file.as_ref())?;
            let mut current_ns = String::new();
            for line in content.lines() {
                let line = line.trim();
                if let Some(ns) = line.strip_prefix("namespace ") {
                    if let Some(end) = ns.find(';') {
                        current_ns = ns[..end].trim().to_string();
                    }
                }
                if line.starts_with("table ") || line.starts_with("struct ") {
                    let rest = &line[6..];
                    if let Some(end) = rest.find('{') {
                        let name = rest[..end].trim();
                        defined_types
                            .entry(current_ns.clone())
                            .or_default()
                            .insert(name.to_string());
                    }
                }
            }
        }
        Ok(defined_types)
    }
}
