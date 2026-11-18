//! Generation-index handle arena.
//!
//! Allocate, get, and free are amortized O(1). Stale handles are rejected.
//! Capacity is fixed at construction; callers cannot grow the live set past it.

/// Opaque generation-index handle. Copying a handle does not keep the slot alive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Handle {
    index: u32,
    generation: u32,
}

impl Handle {
    pub fn index(self) -> u32 {
        self.index
    }

    pub fn generation(self) -> u32 {
        self.generation
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArenaError {
    CapacityExceeded,
    StaleHandle,
}

struct Slot<T> {
    generation: u32,
    value: Option<T>,
}

/// Bounded generation-index arena.
pub struct HandleArena<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
    live: usize,
    capacity: usize,
}

impl<T> HandleArena<T> {
    pub fn bounded(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            live: 0,
            capacity,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.live
    }

    pub fn is_empty(&self) -> bool {
        self.live == 0
    }

    pub fn insert(&mut self, value: T) -> Result<Handle, ArenaError> {
        if self.live >= self.capacity {
            return Err(ArenaError::CapacityExceeded);
        }
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            let generation = slot.generation;
            slot.value = Some(value);
            self.live += 1;
            return Ok(Handle { index, generation });
        }
        if self.slots.len() >= self.capacity {
            return Err(ArenaError::CapacityExceeded);
        }
        let index = u32::try_from(self.slots.len()).map_err(|_| ArenaError::CapacityExceeded)?;
        self.slots.push(Slot {
            generation: 1,
            value: Some(value),
        });
        self.live += 1;
        Ok(Handle {
            index,
            generation: 1,
        })
    }

    pub fn get(&self, handle: Handle) -> Option<&T> {
        let slot = self.slots.get(handle.index as usize)?;
        if slot.generation != handle.generation {
            return None;
        }
        slot.value.as_ref()
    }

    pub fn get_mut(&mut self, handle: Handle) -> Option<&mut T> {
        let slot = self.slots.get_mut(handle.index as usize)?;
        if slot.generation != handle.generation {
            return None;
        }
        slot.value.as_mut()
    }

    pub fn free(&mut self, handle: Handle) -> Result<T, ArenaError> {
        let slot = self
            .slots
            .get_mut(handle.index as usize)
            .ok_or(ArenaError::StaleHandle)?;
        if slot.generation != handle.generation || slot.value.is_none() {
            return Err(ArenaError::StaleHandle);
        }
        let value = slot.value.take().ok_or(ArenaError::StaleHandle)?;
        let next_generation = slot.generation.wrapping_add(1);
        if next_generation == 0 {
            // Fail closed on generation wrap: retire the slot instead of reusing it.
            slot.generation = 0;
        } else {
            slot.generation = next_generation;
            self.free.push(handle.index);
        }
        self.live -= 1;
        Ok(value)
    }
}
