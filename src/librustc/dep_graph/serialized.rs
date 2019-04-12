use rustc_data_structures::sync::worker::{Worker, WorkerExecutor};
use rustc_data_structures::{unlikely, cold_path};
use rustc_data_structures::indexed_vec::{IndexVec, Idx};
use super::graph::{DepNodeIndex, DepNodeData};
use crate::dep_graph::DepNode;
use crate::ich::Fingerprint;

newtype_index! {
    pub struct SerializedDepNodeIndex { .. }
}

/// Data for use when recompiling the **current crate**.
#[derive(Debug, RustcEncodable, RustcDecodable, Default)]
pub struct SerializedDepGraph {
    /// Maps DepNodeIndexes to the index they are stored at
    pub index_to_serial: IndexVec<DepNodeIndex, SerializedDepNodeIndex>,
    pub nodes: IndexVec<SerializedDepNodeIndex, SerializedNode>,
}

#[derive(Debug, RustcEncodable, RustcDecodable)]
pub struct SerializedNode {
    pub node: DepNode,
    pub deps: Vec<DepNodeIndex>,
    pub fingerprint: Fingerprint,
}

#[derive(Debug, RustcEncodable, RustcDecodable, Default)]
struct SerializerWorker {
    graph: SerializedDepGraph,
}

impl SerializerWorker {
    fn write_index(&mut self, index: DepNodeIndex, value: SerializedDepNodeIndex) {
        if unlikely!(self.graph.index_to_serial.len() <= index.as_usize()) {
            cold_path(|| {
                self.graph.index_to_serial.resize(
                    index.as_usize() + 500,
                    SerializedDepNodeIndex::new(DepNodeIndex::INVALID.as_usize()),
                );
            });
        }
        self.graph.index_to_serial[index] = value;
    }
}

impl Worker for SerializerWorker {
    type Message = (DepNodeIndex, DepNodeData);
    type Result = SerializedDepGraph;

    fn message(&mut self, (index, data): (DepNodeIndex, DepNodeData)) {
        let serial_index = SerializedDepNodeIndex::new(self.graph.nodes.len());
        self.write_index(index, serial_index);
        self.graph.nodes.push(SerializedNode {
            node: data.node,
            deps: data.edges.into_iter().collect(),
            fingerprint: data.fingerprint,
        });
    }

    fn complete(self) -> SerializedDepGraph {
        self.graph
    }
}

pub struct Serializer {
    worker: WorkerExecutor<SerializerWorker>,
}

impl Serializer {
    pub fn new(prev_node_count: usize) -> Self {
        Serializer {
            worker: WorkerExecutor::new(SerializerWorker {
                graph: SerializedDepGraph {
                    index_to_serial: (0..prev_node_count).map(|_| {
                        SerializedDepNodeIndex::new(DepNodeIndex::INVALID.as_usize())
                    }).collect(),
                    nodes: IndexVec::with_capacity(prev_node_count),
                }
            })
        }
    }

    pub fn complete(&self) -> SerializedDepGraph {
        self.worker.complete()
    }
}
