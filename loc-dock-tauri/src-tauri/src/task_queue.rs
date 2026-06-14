use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

struct TaskInfo {
    name: String,
    started_at: Instant,
}

#[derive(Serialize, Clone)]
pub struct ActiveTask {
    pub id: u64,
    pub name: String,
    pub elapsed_ms: u64,
}

pub struct TaskQueue {
    inner: Mutex<TaskQueueInner>,
}

struct TaskQueueInner {
    next_id: u64,
    active: HashMap<u64, TaskInfo>,
}

impl TaskQueue {
    pub fn new() -> Self {
        TaskQueue {
            inner: Mutex::new(TaskQueueInner {
                next_id: 1,
                active: HashMap::new(),
            }),
        }
    }

    pub fn start(&self, name: &str) -> u64 {
        let mut inner = self.inner.lock().unwrap();
        let id = inner.next_id;
        inner.next_id += 1;
        inner.active.insert(id, TaskInfo {
            name: name.to_string(),
            started_at: Instant::now(),
        });
        id
    }

    pub fn complete(&self, id: u64) -> Option<u64> {
        let mut inner = self.inner.lock().unwrap();
        inner.active.remove(&id).map(|info| info.started_at.elapsed().as_millis() as u64)
    }

    pub fn active_tasks(&self) -> Vec<ActiveTask> {
        let inner = self.inner.lock().unwrap();
        inner.active.iter().map(|(&id, info)| ActiveTask {
            id,
            name: info.name.clone(),
            elapsed_ms: info.started_at.elapsed().as_millis() as u64,
        }).collect()
    }
}
