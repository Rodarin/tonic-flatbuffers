use regex::Regex;
use std::{
    collections::HashMap,
    fs, io,
    path::Path,
    rc::{Rc, Weak},
};

#[derive(Clone, Debug)]
pub enum NamespaceChild {
    Namespace(Rc<NamespaceNode>),
    Service(Rc<ServiceNode>),
    Type(Rc<TypeNode>),
}

#[derive(Clone, Debug)]
pub struct NamespaceNode {
    pub name: String,
    pub parent: Option<Weak<NamespaceNode>>,
    pub children: Vec<NamespaceChild>,
}

#[derive(Debug)]
pub struct TypeNode {
    pub name: String,
    pub parent: Option<Weak<NamespaceNode>>,
}

#[derive(Debug)]
pub struct ServiceNode {
    pub name: String,
    pub parent: Option<Weak<NamespaceNode>>,
    pub endpoints: Vec<Rc<EndpointNode>>,
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

// Implement TreeNode for NamespaceNode, TypeNode, ServiceNode
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

// Optionally, implement a trait for EndpointNode to get its full path via its service parent
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
}

impl Forest {
    pub fn get_type(&self, fq_name: &str) -> Option<Rc<TypeNode>> {
        self.type_lookup.get(fq_name).cloned()
    }

    /// Build a Forest from a list of .fbs files
    pub fn from_fbs_files(fbs_files: &[impl AsRef<Path>]) -> io::Result<Self> {
        let mut namespace_map: HashMap<String, Rc<NamespaceNode>> = HashMap::new();
        let mut type_lookup: HashMap<String, Rc<TypeNode>> = HashMap::new();

        // Regex declarations for parsing
        let namespace_re = Regex::new(r"(?m)^\s*namespace\s+([a-zA-Z0-9_.]+)\s*;").unwrap();
        let type_re = Regex::new(r"\b(?:table|struct)\s+(\w+)\s*\{").unwrap();
        let service_re = Regex::new(r"rpc_service\s+(\w+)\s*\{([^}]*)\}").unwrap();
        let method_re = Regex::new(
            r"(?x)
                (\w+)\s*                # method name
                \(\s*(\w+)\s*\)\s*      # request type
                :\s*(\w+)\s*            # response type
                \(\s*streaming\s*:\s*(\w+)\s*\)\s*; # streaming
            ",
        )
        .unwrap();

        for file in fbs_files {
            let schema = fs::read_to_string(file.as_ref())?;
            let mut current_ns = String::new();
            let mut cursor = 0;

            while cursor < schema.len() {
                if let Some(ns_cap) = namespace_re.captures(&schema[cursor..]) {
                    let start = schema[cursor..].find(&ns_cap[0]).unwrap();
                    cursor += start + ns_cap[0].len();
                    current_ns = ns_cap[1].to_string();

                    // Build namespace chain (foo.bar.baz)
                    let mut parent: Option<Rc<NamespaceNode>> = None;
                    let mut ns_path = String::new();
                    for segment in current_ns.split('.') {
                        if !ns_path.is_empty() {
                            ns_path.push('.');
                        }
                        ns_path.push_str(segment);

                        let ns_rc = namespace_map
                            .entry(ns_path.clone())
                            .or_insert_with(|| {
                                Rc::new(NamespaceNode {
                                    name: segment.to_string(),
                                    parent: parent.as_ref().map(|p| Rc::downgrade(p)),
                                    children: Vec::new(),
                                })
                            })
                            .clone();

                        parent = Some(ns_rc);
                    }
                    continue;
                }

                // Match types (table/struct)
                if let Some(table_cap) = type_re.captures(&schema[cursor..]) {
                    let start = schema[cursor..].find(&table_cap[0]).unwrap();
                    cursor += start + table_cap[0].len();
                    let fq_name = if current_ns.is_empty() {
                        table_cap[1].to_string()
                    } else {
                        format!("{}.{}", current_ns, table_cap[1].to_string())
                    };
                    let ns_rc = namespace_map.get(&current_ns).unwrap().clone();
                    let type_node = Rc::new(TypeNode {
                        name: table_cap[1].to_string(),
                        parent: Some(Rc::downgrade(&ns_rc)),
                    });
                    type_lookup.insert(fq_name, type_node.clone());
                    // Insert into namespace children
                    Rc::get_mut(&mut Rc::clone(&ns_rc))
                        .unwrap()
                        .children
                        .push(NamespaceChild::Type(type_node));
                    continue;
                }

                // Match rpc_service
                if let Some(svc_cap) = service_re.captures(&schema[cursor..]) {
                    let start = schema[cursor..].find(&svc_cap[0]).unwrap();
                    cursor += start + svc_cap[0].len();
                    let service_name = &svc_cap[1];
                    let body = &svc_cap[2];

                    let ns_rc = namespace_map.get(&current_ns).unwrap().clone();
                    let service_node = Rc::new(ServiceNode {
                        name: service_name.to_string(),
                        parent: Some(Rc::downgrade(&ns_rc)),
                        endpoints: Vec::new(),
                    });

                    for method_cap in method_re.captures_iter(body) {
                        let method = &method_cap[1];
                        let request = &method_cap[2];
                        let response = &method_cap[3];
                        let streaming = &method_cap[4];
                        let endpoint_node = Rc::new(EndpointNode {
                            name: method.to_string(),
                            parent: Some(Rc::downgrade(&service_node)),
                            request_type: type_lookup.get(request).cloned().unwrap_or_else(|| {
                                Rc::new(TypeNode {
                                    name: request.to_string(),
                                    parent: Some(Rc::downgrade(&ns_rc)),
                                })
                            }),
                            response_type: type_lookup.get(response).cloned().unwrap_or_else(
                                || {
                                    Rc::new(TypeNode {
                                        name: response.to_string(),
                                        parent: Some(Rc::downgrade(&ns_rc)),
                                    })
                                },
                            ),
                            streaming: streaming.to_string(),
                        });
                        Rc::get_mut(&mut Rc::clone(&service_node))
                            .expect("ServiceNode is not unique")
                            .endpoints
                            .push(endpoint_node);
                    }

                    continue;
                }

                // Move to next line if nothing matches
                if let Some(next_line) = schema[cursor..].find('\n') {
                    cursor += next_line + 1;
                } else {
                    break;
                }
            }
        }

        // Collect roots (namespaces with no parent)
        let roots = namespace_map
            .values()
            .filter(|ns| ns.parent.is_none())
            .cloned()
            .collect();

        Ok(Forest { roots, type_lookup })
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
            // Push children if Namespace
            if let NamespaceChild::Namespace(ns) = &node {
                for child in ns.children.iter().rev() {
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
        // Start with an owned Rc to self
        let mut current = Some(Rc::new(self.clone()));
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
