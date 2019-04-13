use rustc_data_structures::sync::worker::{Worker, WorkerExecutor};
use rustc_data_structures::{unlikely, cold_path};
use rustc_data_structures::indexed_vec::{IndexVec, Idx};
use rustc_serialize::opaque;
use rustc_serialize::{Decodable, Decoder, Encodable, Encoder};
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
#[derive(Debug, Default)]
pub struct SerializedDepGraph {
    /// Maps DepNodeIndexes to the index they are stored at
    // FIXME: Get rid of this by streaming nodes inside the interner lock,
    // ensuring we can read all dependencies when writing?
    pub nodes: IndexVec<SerializedDepNodeIndex, SerializedNode>,
    pub index_to_serial: IndexVec<DepNodeIndex, SerializedDepNodeIndex>,
}

impl Decodable for SerializedDepGraph {
    fn decode<D: Decoder>(d: &mut D) -> Result<Self, D::Error> {
        let mut nodes = IndexVec::new();
        loop {
            let count = d.read_usize()?;
            if count == 0 {
                break;
            }
            for _ in 0..count {
                nodes.push(SerializedNode::decode(d)?);
            }
        }
        let index_to_serial = IndexVec::decode(d)?;
        Ok(SerializedDepGraph {
            nodes,
            index_to_serial,
        })
    }
}

#[derive(Debug, RustcDecodable)]
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
        use std::time::Instant;
        let now = Instant::now();
        let mut encoder = opaque::Encoder::new(Vec::with_capacity(nodes.len() * BYTES_PER_NODE));
        assert!(!nodes.is_empty());
        encoder.emit_usize(nodes.len());
        for (index, data) in nodes {
            let serial_index = SerializedDepNodeIndex::from_u32(self.count);
            self.count += 1;
            self.write_index(index, serial_index);
            data.node.encode(&mut encoder);
            data.edges.encode(&mut encoder);
            data.fingerprint.encode(&mut encoder);
        }
        self.file.write_all(&encoder.into_inner()).expect("unable to write to temp dep graph");
        eprintln!("SerializerWorker {}", now.elapsed().as_secs_f32());
    }

    fn complete(mut self) {
        use std::time::Instant;
        let now = Instant::now();
        let bytes = self.index_to_serial.len() * mem::size_of::<SerializedDepNodeIndex>();
        let mut encoder = opaque::Encoder::new(Vec::with_capacity(bytes));
        encoder.emit_usize(0);
        self.index_to_serial.encode(&mut encoder);
        self.file.write_all(&encoder.into_inner()).expect("unable to write to temp dep graph");
        eprintln!("SerializerWorker final {}", now.elapsed().as_secs_f32());
    }
}

const BUFFER_SIZE: usize = 70000;
const BYTES_PER_NODE: usize =
    mem::size_of::<DepNodeData>() + mem::size_of::<SerializedDepNodeIndex>();

pub struct Serializer {
    worker: Arc<WorkerExecutor<SerializerWorker>>,
    buffer: Vec<(DepNodeIndex, DepNodeData)>,
}

impl Serializer {
    pub fn new(prev_node_count: usize, path: &Path) -> Self {
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
        if self.buffer.len() > 0 {
            self.flush();
        }
        self.worker.complete()
    }
}
