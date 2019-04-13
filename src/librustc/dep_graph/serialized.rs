use rustc_data_structures::sync::worker::{Worker, WorkerExecutor};
use rustc_data_structures::{unlikely, cold_path};
use rustc_data_structures::indexed_vec::{IndexVec, Idx};
use rustc_serialize::opaque::Encoder;
use rustc_serialize::Encodable;
use std::sync::Arc;
use std::mem;
use std::path::Path;
use std::fs::{File, OpenOptions};
use std::io::Write;
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
    // FIXME: Get rid of this by streaming nodes inside the interner lock,
    // ensuring we can read all dependencies when writing?
    pub nodes: IndexVec<SerializedDepNodeIndex, SerializedNode>,
    pub index_to_serial: IndexVec<DepNodeIndex, SerializedDepNodeIndex>,
}

#[derive(Debug, RustcEncodable, RustcDecodable)]
pub struct SerializedNode {
    pub node: DepNode,
    pub deps: Vec<DepNodeIndex>,
    pub fingerprint: Fingerprint,
}

struct SerializerWorker {
    index_to_serial: IndexVec<DepNodeIndex, SerializedDepNodeIndex>,
    count: u32,
    file: File,
}

impl SerializerWorker {
    fn write_index(&mut self, index: DepNodeIndex, value: SerializedDepNodeIndex) {
        if unlikely!(self.index_to_serial.len() <= index.as_usize()) {
            cold_path(|| {
                self.index_to_serial.resize(
                    index.as_usize() + 500,
                    SerializedDepNodeIndex::new(DepNodeIndex::INVALID.as_usize()),
                );
            });
        }
        self.index_to_serial[index] = value;
    }
}

impl Worker for SerializerWorker {
    type Message = Vec<(DepNodeIndex, DepNodeData)>;
    type Result = ();

    fn message(&mut self, nodes: Vec<(DepNodeIndex, DepNodeData)>) {
        let now = Instant::now();
        let mut encoder = Encoder::new(Vec::with_capacity(nodes.len() * BYTES_PER_NODE));
        for (index, data) in nodes {
            let serial_index = SerializedDepNodeIndex::from_u32(self.count);
            self.count += 1;
            self.write_index(index, serial_index);
            SerializedNode {
                node: data.node,
                deps: data.edges.into_iter().collect(),
                fingerprint: data.fingerprint,
            }.encode(&mut encoder);
        }
        self.file.write_all(&encoder.into_inner()).expect("unable to write to temp dep graph");
        print_time_passes_entry(true, "SerializerWorker", now.elapsed());
    }

    fn complete(self) {
        self.file.sync_data().expect("unable to write to temp dep graph");
    }
}

const BUFFER_SIZE: usize = 90000;
const BYTES_PER_NODE: usize =
    mem::size_of::<DepNodeData>() + mem::size_of::<SerializedDepNodeIndex>();

pub struct Serializer {
    worker: Arc<WorkerExecutor<SerializerWorker>>,
    buffer: Vec<(DepNodeIndex, DepNodeData)>,
}

impl Serializer {
    pub fn new(prev_node_count: usize, path: &Path) -> Self {
    eprintln!("opening dep graph file = {:?}", path);

        let file = OpenOptions::new()
            .write(true)
            .open(path)
            .expect("unable to open temp dep graph file");
        Serializer {
            worker: Arc::new(WorkerExecutor::new(SerializerWorker {
                index_to_serial: (0..prev_node_count).map(|_| {
                    SerializedDepNodeIndex::new(DepNodeIndex::INVALID.as_usize())
                }).collect(),
                file,
                count: 0,
            })),
            buffer: Vec::with_capacity(BUFFER_SIZE),
        }
    }

    fn flush(&mut self) {
        let msgs = mem::replace(&mut self.buffer, Vec::with_capacity(BUFFER_SIZE));
        self.worker.message_in_pool(msgs);
    }

    #[inline]
    pub(super) fn serialize(&mut self, index: DepNodeIndex, data: DepNodeData) {
        self.buffer.push((index, data));
        if unlikely!(self.buffer.len() >= BUFFER_SIZE) {
            cold_path(|| {
                self.flush();
            })
        }
    }

    pub fn complete(&mut self) {
        self.flush();
        self.worker.complete()
    }
}
