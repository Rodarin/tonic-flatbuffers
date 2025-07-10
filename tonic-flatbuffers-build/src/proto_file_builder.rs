use crate::tree::{Forest, NamespaceChild, TypeNode};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
    rc::Rc,
};

pub struct ProtoFileBuilder<'a> {
    forest: &'a Forest,
    out_dir: PathBuf,
    type_usage: HashMap<String, HashSet<Rc<TypeNode>>>,
}

impl<'a> ProtoFileBuilder<'a> {
    pub fn new(forest: &'a Forest, out_dir: impl Into<PathBuf>) -> Self {
        Self {
            forest,
            out_dir: out_dir.into(),
            type_usage: HashMap::new(),
        }
    }

    pub fn build_all(&mut self) -> std::io::Result<()> {
        self.collect_type_usage();
        self.write_type_files()?;
        self.write_service_files()?;
        Ok(())
    }

    fn collect_type_usage(&mut self) {
        for node in self.forest.iter() {
            if let NamespaceChild::Service(service) = node {
                for endpoint in service.endpoints.borrow().iter() {
                    for t in [&endpoint.request_type, &endpoint.response_type] {
                        let ns_path = t.path_segments().join(".");
                        self.type_usage
                            .entry(ns_path)
                            .or_default()
                            .insert(t.clone());
                    }
                }
            }
        }
    }

    fn write_type_files(&self) -> std::io::Result<()> {
        for (ns_path, types) in &self.type_usage {
            let mut file = String::from("syntax = \"proto3\";\n");
            file += &format!("package {}\n\n", ns_path);

            for t in types {
                file += &format!("message {} {{}}\n\n", t.name);
            }
            
            let path = self
                .out_dir
                .join(format!("fake_{}.proto", ns_path.replace('.', "_")));
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(path, file)?;
        }
        Ok(())
    }

    fn write_service_files(&self) -> std::io::Result<()> {
        for node in self.forest.iter() {
            if let NamespaceChild::Service(service) = node {
                let svc_ns = service.path_segments().join(".");

                let mut file = String::from("syntax = \"proto3\";\n");

                // collect imports
                let mut imports = HashSet::new();
                for ep in service.endpoints.borrow().iter() {
                    for t in [&ep.request_type, &ep.response_type] {
                        let ns = t.path_segments().join(".");
                        if ns != svc_ns {
                            imports.insert(ns);
                        }
                    }
                }

                for ns in &imports {
                    let path = format!("fake_{}.proto", ns.replace('.', "_"));
                    file += &format!("import \"{}\";\n", path);
                }

                file += &format!("\npackage {};\n\n", svc_ns);
                file += &format!("service {} {{\n", service.name);

                for ep in service.endpoints.borrow().iter() {
                    let req_ns = ep.request_type.path_segments().join(".");
                    let res_ns = ep.response_type.path_segments().join(".");
                    let req = if req_ns != svc_ns {
                        format!("{}.{}", req_ns, ep.request_type.name)
                    } else {
                        ep.request_type.name.clone()
                    };
                    let res = if res_ns != svc_ns {
                        format!("{}.{}", res_ns, ep.response_type.name)
                    } else {
                        ep.response_type.name.clone()
                    };

                    file += &format!("    rpc {} ({}) returns ({}) {{}}\n", ep.name, req, res);
                }
                
                file += "}\n";
                let path = self
                    .out_dir
                    .join(format!("fake_{}.proto", svc_ns.replace('.', "_")));
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(path, file)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::Forest;
    use std::{
        fs::{read_to_string, File},
        io::Write,
    };
    use tempfile::tempdir;

    fn write_schema(path: &std::path::Path, contents: &str) {
        File::create(path)
            .unwrap()
            .write_all(contents.as_bytes())
            .unwrap();
    }

    #[test]
    fn generates_protos_for_cross_namespace_usage() {
        let dir = tempdir().unwrap();
        let types_path = dir.path().join("types.fbs");
        let services_path = dir.path().join("services.fbs");
        let out_dir = dir.path().join("out");
        std::fs::create_dir(&out_dir).unwrap();

        let schema_types = r#"
            namespace foo.shared;
            table SharedReq {}
            table SharedRes {}
        "#;

        let schema_services = r#"
            namespace foo.service;
            rpc_service CrossNsService {
                UseShared(foo.shared.SharedReq): foo.shared.SharedRes (streaming: none);
            }
        "#;

        write_schema(&types_path, schema_types);
        write_schema(&services_path, schema_services);

        let forest = Forest::from_fbs_files(&[types_path, services_path]).unwrap();
        let mut builder = ProtoFileBuilder::new(&forest, &out_dir);
        builder.build_all().unwrap();

        let shared_proto = read_to_string(out_dir.join("fake_foo_shared.proto")).unwrap();
        assert!(shared_proto.contains("package foo.shared"));
        assert!(shared_proto.contains("message SharedReq"));
        assert!(shared_proto.contains("message SharedRes"));

        let service_proto = read_to_string(out_dir.join("fake_foo_service.proto")).unwrap();
        assert!(service_proto.contains("import \"fake_foo_shared.proto\""));
        assert!(service_proto.contains("package foo.service"));
        assert!(service_proto.contains("service CrossNsService"));
        assert!(service_proto
            .contains("rpc UseShared (foo.shared.SharedReq) returns (foo.shared.SharedRes)"));
    }

    #[test]
    fn generates_protos_for_single_namespace() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("single.fbs");
        let out_dir = dir.path().join("out");
        std::fs::create_dir(&out_dir).unwrap();

        let schema = r#"
            namespace my.api;
            table MyReq {}
            table MyRes {}
            rpc_service LocalSvc {
                Ping(MyReq): MyRes (streaming: none);
            }
        "#;

        write_schema(&file_path, schema);

        let forest = Forest::from_fbs_files(&[file_path]).unwrap();
        let mut builder = ProtoFileBuilder::new(&forest, &out_dir);
        builder.build_all().unwrap();

        let proto_file = read_to_string(out_dir.join("fake_my_api.proto")).unwrap();
        assert!(proto_file.contains("package my.api"));
        assert!(proto_file.contains("message MyReq"));
        assert!(proto_file.contains("message MyRes"));
        assert!(proto_file.contains("service LocalSvc"));
        assert!(proto_file.contains("rpc Ping (MyReq) returns (MyRes)"));
    }

    #[test]
    fn does_not_generate_type_file_if_type_not_used_in_service() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("unused.fbs");
        let out_dir = dir.path().join("out");
        std::fs::create_dir(&out_dir).unwrap();

        let schema = r#"
            namespace lonely.types;
            table Orphaned {} // not referenced
        "#;

        write_schema(&file_path, schema);

        let forest = Forest::from_fbs_files(&[file_path]).unwrap();
        let mut builder = ProtoFileBuilder::new(&forest, &out_dir);
        builder.build_all().unwrap();

        assert!(out_dir.read_dir().unwrap().next().is_none()); // no files created
    }
}
