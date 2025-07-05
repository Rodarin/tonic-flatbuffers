use std::io;
use std::path::Path;
use std::rc::{Rc, Weak};

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

use std::collections::HashMap;

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
        use std::collections::HashMap;
        use std::fs;

        let mut namespace_map: HashMap<String, Rc<NamespaceNode>> = HashMap::new();
        let mut type_lookup: HashMap<String, Rc<TypeNode>> = HashMap::new();

        for file in fbs_files {
            let content = fs::read_to_string(file.as_ref())?;
            let mut current_ns = String::new();

            for line in content.lines() {
                let line = line.trim();

                // Parse namespace
                if let Some(ns) = line.strip_prefix("namespace ") {
                    if let Some(end) = ns.find(';') {
                        current_ns = ns[..end].trim().to_string();

                        // Build namespace chain (foo.bar.baz)
                        let mut parent: Option<Rc<NamespaceNode>> = None;
                        let mut ns_path = String::new();
                        for segment in current_ns.split('.') {
                            if !ns_path.is_empty() {
                                ns_path.push('.');
                            }
                            ns_path.push_str(segment);

                            let ns_rc = namespace_map.entry(ns_path.clone()).or_insert_with(|| {
                                Rc::new(NamespaceNode {
                                    name: segment.to_string(),
                                    parent: parent.as_ref().map(|p| Rc::downgrade(p)),
                                    children: Vec::new(),
                                })
                            }).clone();

                            parent = Some(ns_rc);
                        }
                    }
                }

                // Parse types (table/struct)
                if line.starts_with("table ") || line.starts_with("struct ") {
                    let rest = &line[6..];
                    if let Some(end) = rest.find('{') {
                        let name = rest[..end].trim();
                        let fq_name = if current_ns.is_empty() {
                            name.to_string()
                        } else {
                            format!("{}.{}", current_ns, name)
                        };
                        let ns_rc = namespace_map.get(&current_ns).unwrap().clone();
                        let type_node = Rc::new(TypeNode {
                            name: name.to_string(),
                            parent: Some(Rc::downgrade(&ns_rc)),
                        });
                        type_lookup.insert(fq_name, type_node.clone());
                        // Insert into namespace children
                        Rc::get_mut(&mut Rc::clone(&ns_rc))
                            .unwrap()
                            .children
                            .push(NamespaceChild::Type(type_node));
                    }
                }

                // Parse services (very naive, assumes "rpc_service Name { ... }")
                if line.starts_with("rpc_service ") {
                    let rest = &line[12..];
                    if let Some(end) = rest.find('{') {
                        let name = rest[..end].trim();
                        let ns_rc = namespace_map.get(&current_ns).unwrap().clone();
                        let service_node = Rc::new(ServiceNode {
                            name: name.to_string(),
                            parent: Some(Rc::downgrade(&ns_rc)),
                            endpoints: Vec::new(), // TODO: parse endpoints
                        });
                        Rc::get_mut(&mut Rc::clone(&ns_rc))
                            .unwrap()
                            .children
                            .push(NamespaceChild::Service(service_node));
                    }
                }
                // TODO: Parse endpoints and attach to services
            }
        }

        // Collect roots (namespaces with no parent)
        let roots = namespace_map
            .values()
            .filter(|ns| ns.parent.is_none())
            .cloned()
            .collect();

        Ok(Forest {
            roots,
            type_lookup,
        })
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
