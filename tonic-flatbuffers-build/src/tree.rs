use regex::Regex;
use std::{
    cell::RefCell,
    collections::HashMap,
    fs,
    hash::{Hash, Hasher},
    io,
    path::Path,
    rc::{Rc, Weak},
};

#[derive(Clone, Debug)]
pub enum NamespaceChild {
    Namespace(Rc<NamespaceNode>),
    Service(Rc<ServiceNode>),
    Type(Rc<TypeNode>),
}

#[derive(Debug)]
pub struct NamespaceNode {
    pub name: String,
    pub parent: Option<Weak<NamespaceNode>>,
    pub children: RefCell<Vec<NamespaceChild>>,
}

#[derive(Debug)]
pub struct TypeNode {
    pub name: String,
    pub parent: Option<Weak<NamespaceNode>>,
}

impl PartialEq for TypeNode {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for TypeNode {}

impl Hash for TypeNode {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

#[derive(Debug)]
pub struct ServiceNode {
    pub name: String,
    pub parent: Option<Weak<NamespaceNode>>,
    pub endpoints: RefCell<Vec<Rc<EndpointNode>>>,
}

#[derive(Debug)]
pub struct EndpointNode {
    pub name: String,
    pub parent: Option<Weak<ServiceNode>>,
    pub request_type: Rc<TypeNode>,
    pub response_type: Rc<TypeNode>,
    pub streaming: String,
}

pub trait TreeNode {
    fn name(&self) -> &str;
    fn parent_namespace(&self) -> Option<Weak<NamespaceNode>>;
}

impl TreeNode for NamespaceNode {
    fn name(&self) -> &str {
        &self.name
    }
    fn parent_namespace(&self) -> Option<Weak<NamespaceNode>> {
        self.parent.clone()
    }
}

impl TreeNode for TypeNode {
    fn name(&self) -> &str {
        &self.name
    }
    fn parent_namespace(&self) -> Option<Weak<NamespaceNode>> {
        self.parent.clone()
    }
}

impl TreeNode for ServiceNode {
    fn name(&self) -> &str {
        &self.name
    }
    fn parent_namespace(&self) -> Option<Weak<NamespaceNode>> {
        self.parent.clone()
    }
}

impl EndpointNode {
    pub fn path(&self) -> String {
        if let Some(ref weak) = self.parent {
            if let Some(service) = weak.upgrade() {
                format!("{}.{}", service.path(), self.name)
            } else {
                self.name.clone()
            }
        } else {
            self.name.clone()
        }
    }
}

pub struct Forest {
    pub roots: Vec<Rc<NamespaceNode>>,
    pub type_lookup: HashMap<String, Rc<TypeNode>>,
    pub namespace_map: HashMap<String, Rc<NamespaceNode>>,
}

impl Forest {
    pub fn get_type(&self, fq_name: &str) -> Option<Rc<TypeNode>> {
        self.type_lookup.get(fq_name).cloned()
    }

    pub fn from_fbs_files(fbs_files: &[impl AsRef<Path>]) -> io::Result<Self> {
        let mut namespace_map: HashMap<String, Rc<NamespaceNode>> = HashMap::new();
        let mut type_lookup: HashMap<String, Rc<TypeNode>> = HashMap::new();

        // Regex declarations for parsing
        let namespace_re = Regex::new(r"(?m)^\s*namespace\s+([a-zA-Z0-9_.]+)\s*;").unwrap();
        let service_re = Regex::new(r"(?s)rpc_service\s+(\w+)\s*\{(.*?)\}").unwrap();
        let method_re = Regex::new(
            r"(?x)
                (\w+)\s*            # method name
                \(\s*(\S+)\s*\)\s*  # request type
                :\s*(\S+)\s*        # response type
                \(\s*streaming:\s*(\w+)\s*\)\s*;
            ",
        )
        .unwrap();

        for file in fbs_files {
            let schema = fs::read_to_string(file.as_ref())?;
            let mut current_ns = String::new();
            let mut cursor = 0;

            while cursor < schema.len() {
                let slice = &schema[cursor..];
                let mut progress_made = false;

                if let Some(ns_cap) = namespace_re.captures(slice) {
                    let start = slice.find(&ns_cap[0]).unwrap();
                    cursor += start + ns_cap[0].len();
                    current_ns = ns_cap[1].to_string();
                    Self::ensure_namespace_chain(&mut namespace_map, &current_ns);
                    progress_made = true;
                } else if let Some(svc_cap) = service_re.captures(slice) {
                    let start = slice.find(&svc_cap[0]).unwrap();
                    cursor += start + svc_cap[0].len();

                    let service_name = &svc_cap[1];
                    let body = &svc_cap[2];

                    let ns_rc = namespace_map.get(&current_ns).unwrap().clone();
                    let service_node = Rc::new(ServiceNode {
                        name: service_name.to_string(),
                        parent: Some(Rc::downgrade(&ns_rc)),
                        endpoints: RefCell::new(Vec::new()),
                    });

                    for method_cap in method_re.captures_iter(body) {
                        let method = &method_cap[1];
                        let request = &method_cap[2];
                        let response = &method_cap[3];
                        let streaming = &method_cap[4];

                        let request_type =
                            Self::get_or_create_type(&mut type_lookup, &mut namespace_map, request);
                        let response_type = Self::get_or_create_type(
                            &mut type_lookup,
                            &mut namespace_map,
                            response,
                        );

                        let endpoint_node = Rc::new(EndpointNode {
                            name: method.to_string(),
                            parent: Some(Rc::downgrade(&service_node)),
                            request_type,
                            response_type,
                            streaming: streaming.to_string(),
                        });
                        service_node.endpoints.borrow_mut().push(endpoint_node);
                    }

                    ns_rc
                        .children
                        .borrow_mut()
                        .push(NamespaceChild::Service(service_node));
                    progress_made = true;
                }

                if !progress_made {
                    // skip ahead by line to avoid infinite loop
                    if let Some(next_line) = slice.find('\n') {
                        cursor += next_line + 1;
                    } else {
                        break;
                    }
                }
            }
        }

        let roots = namespace_map
            .values()
            .filter(|ns| ns.parent.is_none())
            .cloned()
            .collect();

        Ok(Forest {
            roots,
            type_lookup,
            namespace_map,
        })
    }

    fn ensure_namespace_chain(
        map: &mut HashMap<String, Rc<NamespaceNode>>,
        ns: &str,
    ) -> Rc<NamespaceNode> {
        let mut parent: Option<Rc<NamespaceNode>> = None;
        let mut ns_path = String::new();
        let mut last_rc = None;

        for segment in ns.split('.') {
            if !ns_path.is_empty() {
                ns_path.push('.');
            }
            ns_path.push_str(segment);

            let ns_rc = map
                .entry(ns_path.clone())
                .or_insert_with(|| {
                    Rc::new(NamespaceNode {
                        name: segment.to_string(),
                        parent: parent.as_ref().map(Rc::downgrade),
                        children: RefCell::new(Vec::new()),
                    })
                })
                .clone();

            parent = Some(ns_rc.clone());
            last_rc = Some(ns_rc);
        }
        last_rc.unwrap()
    }

    fn get_or_create_type(
        type_lookup: &mut HashMap<String, Rc<TypeNode>>,
        namespace_map: &mut HashMap<String, Rc<NamespaceNode>>,
        fq_name: &str,
    ) -> Rc<TypeNode> {
        if let Some(t) = type_lookup.get(fq_name) {
            return t.clone();
        }

        let (ns_path, type_name) = match fq_name.rsplit_once('.') {
            Some((ns, name)) => (ns, name),
            None => ("", fq_name),
        };

        let ns_rc = Self::ensure_namespace_chain(namespace_map, ns_path);
        let type_node = Rc::new(TypeNode {
            name: type_name.to_string(),
            parent: Some(Rc::downgrade(&ns_rc)),
        });
        ns_rc
            .children
            .borrow_mut()
            .push(NamespaceChild::Type(type_node.clone()));
        type_lookup.insert(fq_name.to_string(), type_node.clone());
        type_node
    }

    pub fn iter(&self) -> ForestIter {
        let mut stack = Vec::new();
        for root in self.roots.iter().rev() {
            stack.push(NamespaceChild::Namespace(root.clone()));
        }
        ForestIter { stack }
    }
}

pub struct ForestIter {
    stack: Vec<NamespaceChild>,
}

impl Iterator for ForestIter {
    type Item = NamespaceChild;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(node) = self.stack.pop() {
            if let NamespaceChild::Namespace(ns) = &node {
                for child in ns.children.borrow().iter().rev() {
                    self.stack.push(child.clone());
                }
            }
            Some(node)
        } else {
            None
        }
    }
}

impl TypeNode {
    pub fn path_segments(&self) -> Vec<String> {
        let mut segments = Vec::new();
        let mut current = self.parent.as_ref().and_then(|w| w.upgrade());
        while let Some(ns) = current {
            segments.push(ns.name.clone());
            current = ns.parent.as_ref().and_then(|w| w.upgrade());
        }
        segments.reverse();
        segments.push(self.name.clone());
        segments
    }
    pub fn path(&self) -> String {
        self.path_segments().join(".")
    }
}

impl ServiceNode {
    pub fn path_segments(&self) -> Vec<String> {
        let mut segments = Vec::new();
        let mut current = self.parent.as_ref().and_then(|w| w.upgrade());
        while let Some(ns) = current {
            segments.push(ns.name.clone());
            current = ns.parent.as_ref().and_then(|w| w.upgrade());
        }
        segments.reverse();
        segments.push(self.name.clone());
        segments
    }
    pub fn path(&self) -> String {
        self.path_segments().join(".")
    }
}

impl NamespaceNode {
    pub fn path_segments(&self) -> Vec<String> {
        let mut segments = Vec::new();
        let mut current = self.parent.as_ref().and_then(|w| w.upgrade());
        while let Some(ns_rc) = current {
            segments.push(ns_rc.name.clone());
            current = ns_rc.parent.as_ref().and_then(|w| w.upgrade());
        }
        segments.reverse();
        segments
    }
    pub fn path(&self) -> String {
        self.path_segments().join(".")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn parses_single_schema_with_namespace_and_service() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("schema1.fbs");
        let schema = r#"
            namespace foo.bar;

            table Request {}
            table Response {}

            rpc_service MyService {
                Call(Request): Response (streaming: none);
            }
        "#;

        File::create(&file_path)
            .unwrap()
            .write_all(schema.as_bytes())
            .unwrap();

        let forest = Forest::from_fbs_files(&[file_path]).unwrap();
        assert_eq!(forest.roots.len(), 1);
    }

    #[test]
    fn parses_multiple_namespaces_with_common_root() {
        let dir = tempdir().unwrap();
        let file_path1 = dir.path().join("a.fbs");
        let file_path2 = dir.path().join("b.fbs");

        let schema1 = "namespace root.shared; table A {};";
        let schema2 = "namespace root.other; table B {};";

        File::create(&file_path1)
            .unwrap()
            .write_all(schema1.as_bytes())
            .unwrap();
        File::create(&file_path2)
            .unwrap()
            .write_all(schema2.as_bytes())
            .unwrap();

        let forest = Forest::from_fbs_files(&[file_path1, file_path2]).unwrap();
        assert_eq!(forest.roots.len(), 1); // "root"
    }

    #[test]
    fn service_uses_type_from_other_namespace() {
        let dir = tempdir().unwrap();
        let path1 = dir.path().join("types.fbs");
        let path2 = dir.path().join("services.fbs");

        let schema1 = "namespace ns.types; table TReq {}; table TRes {};";
        let schema2 = r#"
            namespace ns.api;
            rpc_service ExternalUser {
                Op(ns.types.TReq): ns.types.TRes (streaming: server);
            }
        "#;

        File::create(&path1)
            .unwrap()
            .write_all(schema1.as_bytes())
            .unwrap();
        File::create(&path2)
            .unwrap()
            .write_all(schema2.as_bytes())
            .unwrap();

        let forest = Forest::from_fbs_files(&[path1, path2]).unwrap();
        let types_found = forest.type_lookup.contains_key("ns.types.TReq")
            && forest.type_lookup.contains_key("ns.types.TRes");
        assert!(types_found);
    }
}
