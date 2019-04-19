use rustc_data_structures::sync::worker::{Worker, WorkerExecutor};
use rustc_data_structures::sync::Lrc;
use rustc_data_structures::{unlikely, cold_path};
use rustc_data_structures::indexed_vec::{IndexVec, Idx};
use rustc_serialize::opaque;
use rustc_serialize::{Decodable, Decoder, Encodable, Encoder};
use std::mem;
use std::fs::File;
use std::io::Write;
use super::graph::{DepNodeData, DepNodeIndex};
use crate::dep_graph::DepNode;
use crate::ich::Fingerprint;

newtype_index! {
    pub struct SerializedDepNodeIndex { .. }
}

impl SerializedDepNodeIndex {
    pub fn current(self) -> DepNodeIndex {
        DepNodeIndex::from_u32(self.as_u32())
    }
}

#[derive(Debug, Default)]
pub struct SerializedDepGraph {
    pub nodes: IndexVec<DepNodeIndex, DepNodeData>,
    pub invalidated: IndexVec<DepNodeIndex, bool>,
}

impl SerializedDepGraph {
    fn decode(d: &mut opaque::Decoder<'_>) -> Result<Self, String> {
        let mut nodes = IndexVec::new();
        let mut invalidated_list = Vec::new();
        loop {
            if d.position() == d.data.len() {
                break;
            }
            match Action::decode(d)? {
                Action::NewNodes(nodes) => {
                    nodes.extend(nodes);
                }
                Action::UpdateNodes(changed) => {
                    for (i, data) in changed {
                        nodes[i] = data;
                    }
                }
                Action::InvalidateNodes(nodes) => {
                    invalidated_list.extend(nodes);
                }
            }
        }
        let mut invalidated = (0..nodes.len()).map(|_| false).collect();
        for i in invalidated_list {
            invalidated[i] = true;
        }
        Ok(SerializedDepGraph {
            nodes,
            invalidated,
        })
    }
}

#[derive(Debug, RustcEncodable, RustcDecodable)]
pub enum Action {
    NewNodes(Vec<DepNodeData>),
    UpdateNodes(Vec<(DepNodeIndex, DepNodeData)>),
    /// FIXME: Is this redundant since these nodes will be also be updated?
    InvalidateNodes(Vec<DepNodeIndex>)
}

struct SerializerWorker {
    file: File,
}

impl Worker for SerializerWorker {
    type Message = (usize, Action);
    type Result = ();

    fn message(&mut self, (buffer_size_est, action): (usize, Vec<DepNodeData>)) {
        let mut encoder = opaque::Encoder::new(Vec::with_capacity(buffer_size_est * 5));
        action.encode(&mut encoder).ok();
        self.file.write_all(&encoder.into_inner()).expect("unable to write to temp dep graph");
    }

    fn complete(mut self) {}
}

const BUFFER_SIZE: usize = 800000;

pub struct Serializer {
    worker: Lrc<WorkerExecutor<SerializerWorker>>,
    new_buffer: Vec<DepNodeData>,
    new_buffer_size: usize,
    updated_buffer: Vec<(DepNodeIndex, DepNodeData)>,
    updated_buffer_size: usize,
}

impl Serializer {
    pub fn new(file: File) -> Self {
        Serializer {
            worker: Lrc::new(WorkerExecutor::new(SerializerWorker {
                file,
            })),
            new_buffer: Vec::with_capacity(BUFFER_SIZE),
            new_buffer_size: 0,
            updated_buffer: Vec::with_capacity(BUFFER_SIZE),
            updated_buffer_size: 0,
        }
    }

    fn flush_new(&mut self) {
        let msgs = mem::replace(&mut self.buffer, Vec::with_capacity(BUFFER_SIZE));
        let buffer_size = self.new_buffer_size;
        self.new_buffer_size = 0;
        self.worker.message_in_pool((buffer_size, Action::NewNodes(msgs)));
    }

    #[inline]
    pub(super) fn serialize_new(&mut self, data: DepNodeData) {
        let edges = data.edges.len();
        self.new_buffer.push(data);
        self.new_buffer_size += 8 + edges;
        if unlikely!(self.new_buffer_size >= BUFFER_SIZE) {
            cold_path(|| {
                self.flush_new();
            })
        }
    }

    fn flush_updated(&mut self) {
        let msgs = mem::replace(&mut self.buffer, Vec::with_capacity(BUFFER_SIZE));
        let buffer_size = self.updated_buffer_size;
        self.updated_buffer_size = 0;
        self.worker.message_in_pool((buffer_size, Action::UpdateNodes(msgs)));
    }

    #[inline]
    pub(super) fn serialize_updated(&mut self, _index: DepNodeIndex, data: DepNodeData) {
        let edges = data.edges.len();
        self.updated_buffer.push(data);
        self.updated_buffer_size += 9 + edges;
        if unlikely!(self.updated_buffer_size >= BUFFER_SIZE) {
            cold_path(|| {
                self.flush_updated();
            })
        }
    }

    pub fn complete(&mut self, data: &DepGraphData) {
        if self.new_buffer.len() > 0 {
            self.flush_new();
        }
        if self.updated_buffer.len() > 0 {
            self.flush_updated();
        }
        data.previous.invalidated
        self.worker.message_in_pool((buffer_size, Action::UpdateNodes(msgs)));
        self.worker.complete()
    }
}
