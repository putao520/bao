// @trace REQ-LIB-001 REQ-LIB-004 [entity:PagePool]
// @trace REQ-BRW-003: Multi-page pool with idle eviction
// @trace REQ-LIB-001: Headless multi-page management API
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use dpi::PhysicalSize;
use servo::Servo;

use crate::config::{BaoConfig, PageConfig};
use crate::delegate::BaoServoDelegate;
use crate::error::BrowserError;
use crate::page::PageHandle;

pub struct PoolStats {
    pub active: usize,
    pub idle: usize,
    pub total_created: usize,
    pub total_destroyed: usize,
}

struct IdleEntry {
    page: PageHandle,
    idle_since: Instant,
}

pub struct PagePool {
    servo: Rc<Servo>,
    servo_delegate: Rc<BaoServoDelegate>,
    active_pages: RefCell<HashMap<usize, PageHandle>>,
    idle_pages: RefCell<HashMap<usize, IdleEntry>>,
    max_total: usize,
    idle_ttl: Duration,
    default_viewport: PhysicalSize<u32>,
    next_id: RefCell<usize>,
    total_created: RefCell<usize>,
    total_destroyed: RefCell<usize>,
}

impl PagePool {
    pub fn new(
        servo: Rc<Servo>,
        servo_delegate: Rc<BaoServoDelegate>,
        config: &BaoConfig,
    ) -> Self {
        PagePool {
            servo,
            servo_delegate,
            active_pages: RefCell::new(HashMap::new()),
            idle_pages: RefCell::new(HashMap::new()),
            max_total: config.max_pages,
            idle_ttl: config.idle_ttl,
            default_viewport: PhysicalSize::new(
                config.default_viewport_width,
                config.default_viewport_height,
            ),
            next_id: RefCell::new(1),
            total_created: RefCell::new(0),
            total_destroyed: RefCell::new(0),
        }
    }

    pub fn create_page(&self, config: &PageConfig) -> Result<PageHandle, BrowserError> {
        let total = self.active_pages.borrow().len() + self.idle_pages.borrow().len();
        if total >= self.max_total {
            return Err(BrowserError::Init(format!(
                "page limit exceeded: {total}/{}",
                self.max_total
            )));
        }

        let id = {
            let mut next = self.next_id.borrow_mut();
            let id = *next;
            *next += 1;
            id
        };

        let page = PageHandle::new(
            Rc::clone(&self.servo),
            Rc::clone(&self.servo_delegate),
            config,
            self.default_viewport,
            id,
        )?;

        self.active_pages.borrow_mut().insert(id, page.clone());
        *self.total_created.borrow_mut() += 1;

        Ok(page)
    }

    pub fn get_page(&self, id: usize) -> Option<PageHandle> {
        if let Some(page) = self.active_pages.borrow().get(&id) {
            return Some(page.clone());
        }
        if let Some(entry) = self.idle_pages.borrow_mut().remove(&id) {
            self.active_pages.borrow_mut().insert(id, entry.page.clone());
            return Some(entry.page);
        }
        None
    }

    pub fn close_page(&self, id: usize) -> Result<(), BrowserError> {
        if let Some(page) = self.active_pages.borrow_mut().remove(&id) {
            page.close()?;
            *self.total_destroyed.borrow_mut() += 1;
            return Ok(());
        }
        if let Some(entry) = self.idle_pages.borrow_mut().remove(&id) {
            entry.page.close()?;
            *self.total_destroyed.borrow_mut() += 1;
            return Ok(());
        }
        Err(BrowserError::Init(format!("page {id} not found")))
    }

    pub fn release_page(&self, id: usize) {
        if let Some(page) = self.active_pages.borrow_mut().remove(&id) {
            self.idle_pages.borrow_mut().insert(
                id,
                IdleEntry {
                    page,
                    idle_since: Instant::now(),
                },
            );
        }
    }

    pub fn check_idle_pages(&self) -> usize {
        let mut reclaimed = 0;
        let expired: Vec<usize> = self
            .idle_pages
            .borrow()
            .iter()
            .filter(|(_, entry)| entry.idle_since.elapsed() > self.idle_ttl)
            .map(|(id, _)| *id)
            .collect();

        for id in expired {
            if let Some(entry) = self.idle_pages.borrow_mut().remove(&id) {
                let _ = entry.page.close();
                *self.total_destroyed.borrow_mut() += 1;
                reclaimed += 1;
            }
        }

        reclaimed
    }

    pub fn stats(&self) -> PoolStats {
        PoolStats {
            active: self.active_pages.borrow().len(),
            idle: self.idle_pages.borrow().len(),
            total_created: *self.total_created.borrow(),
            total_destroyed: *self.total_destroyed.borrow(),
        }
    }

    pub fn close_all(&self) {
        for (_, page) in self.active_pages.borrow_mut().drain() {
            let _ = page.close();
            *self.total_destroyed.borrow_mut() += 1;
        }
        for (_, entry) in self.idle_pages.borrow_mut().drain() {
            let _ = entry.page.close();
            *self.total_destroyed.borrow_mut() += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_stats_construction() {
        let stats = PoolStats {
            active: 3,
            idle: 2,
            total_created: 10,
            total_destroyed: 5,
        };
        assert_eq!(stats.active, 3);
        assert_eq!(stats.idle, 2);
        assert_eq!(stats.total_created, 10);
        assert_eq!(stats.total_destroyed, 5);
    }

    #[test]
    fn pool_stats_zero() {
        let stats = PoolStats {
            active: 0,
            idle: 0,
            total_created: 0,
            total_destroyed: 0,
        };
        assert_eq!(stats.active + stats.idle, 0);
    }

    #[test]
    fn pool_stats_invariant() {
        // total_created >= total_destroyed (can't destroy more than created)
        let stats = PoolStats {
            active: 5,
            idle: 3,
            total_created: 20,
            total_destroyed: 12,
        };
        assert!(stats.total_created >= stats.total_destroyed);
        assert_eq!(stats.active + stats.idle, stats.total_created - stats.total_destroyed);
    }

    #[test]
    fn pool_stats_all_active() {
        let stats = PoolStats {
            active: 8,
            idle: 0,
            total_created: 8,
            total_destroyed: 0,
        };
        assert_eq!(stats.idle, 0);
        assert_eq!(stats.active, stats.total_created);
    }

    #[test]
    fn pool_stats_all_idle() {
        let stats = PoolStats {
            active: 0,
            idle: 4,
            total_created: 4,
            total_destroyed: 0,
        };
        assert_eq!(stats.active, 0);
        assert_eq!(stats.idle, stats.total_created);
    }
}
