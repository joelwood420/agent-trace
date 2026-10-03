//! Collects a stream of trace events into a tree.

use std::collections::HashMap;

use crate::event::TraceEvent;

/// An event that would break the tree.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TraceError {
    /// The event names a parent that has not been sent yet.
    #[error("event {id} has unknown parent {parent_id}")]
    UnknownParent {
        /// The event's id.
        id: String,
        /// The parent id it named.
        parent_id: String,
    },
    /// A re-sent event names a different parent than the first version.
    #[error("event {id} changed parent from {old:?} to {new:?}")]
    ParentChanged {
        /// The event's id.
        id: String,
        /// The parent id of the first version.
        old: Option<String>,
        /// The parent id of the new version.
        new: Option<String>,
    },
    /// A re-sent event has a different node kind than the first version.
    #[error("event {id} changed kind from {old} to {new}")]
    KindChanged {
        /// The event's id.
        id: String,
        /// The kind of the first version.
        old: &'static str,
        /// The kind of the new version.
        new: &'static str,
    },
}

/// The current state of a trace: the latest version of every node, plus
/// the order in which nodes were first seen.
///
/// Children are ordered by when they were first sent. A re-sent node keeps
/// its original position.
#[derive(Debug, Default, Clone)]
pub struct Trace {
    nodes: HashMap<String, TraceEvent>,
    children: HashMap<String, Vec<String>>,
    roots: Vec<String>,
}

impl Trace {
    /// An empty trace.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a new node, or replaces an existing node with the same `id`.
    ///
    /// Fails, and leaves the trace unchanged, if the parent is unknown or if
    /// a re-sent node changes its parent or kind.
    pub fn apply(&mut self, event: TraceEvent) -> Result<(), TraceError> {
        if let Some(existing) = self.nodes.get(&event.id) {
            if existing.parent_id != event.parent_id {
                return Err(TraceError::ParentChanged {
                    id: event.id,
                    old: existing.parent_id.clone(),
                    new: event.parent_id,
                });
            }
            let (old, new) = (existing.node.kind_name(), event.node.kind_name());
            if old != new {
                return Err(TraceError::KindChanged {
                    id: event.id,
                    old,
                    new,
                });
            }
            self.nodes.insert(event.id.clone(), event);
            return Ok(());
        }

        match &event.parent_id {
            Some(parent_id) => {
                if !self.nodes.contains_key(parent_id) {
                    return Err(TraceError::UnknownParent {
                        id: event.id,
                        parent_id: parent_id.clone(),
                    });
                }
                self.children
                    .entry(parent_id.clone())
                    .or_default()
                    .push(event.id.clone());
            }
            None => self.roots.push(event.id.clone()),
        }
        self.nodes.insert(event.id.clone(), event);
        Ok(())
    }

    /// The latest version of the node with this id.
    pub fn get(&self, id: &str) -> Option<&TraceEvent> {
        self.nodes.get(id)
    }

    /// Root nodes (those with no parent), in the order first sent.
    pub fn roots(&self) -> impl Iterator<Item = &TraceEvent> {
        self.roots.iter().filter_map(|id| self.nodes.get(id))
    }

    /// Children of the node with this id, in the order first sent.
    pub fn children(&self, id: &str) -> impl Iterator<Item = &TraceEvent> {
        self.children
            .get(id)
            .into_iter()
            .flatten()
            .filter_map(|child| self.nodes.get(child))
    }

    /// Number of distinct nodes.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// `true` if the trace has no nodes.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}
