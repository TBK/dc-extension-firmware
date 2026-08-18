//! Flash storage for persistent power state and restore mode.
//!
//! Uses a circular buffer in flash to minimize wear. Each entry is stored
//! in a separate flash page, with automatic sector erase when full.

use embassy_rp::flash::{Blocking, ERASE_SIZE, Flash, PAGE_SIZE};
use embassy_rp::peripherals::FLASH;

use crate::config::FLASH_TARGET_OFFSET;

/// Total flash size of the RP2040 board (2 MiB)
pub const FLASH_SIZE: usize = 2 * 1024 * 1024;

/// Blocking flash driver over the on-board flash (no DMA - our reads
/// are memory-mapped XIP and writes are blocking ROM calls)
pub type DcFlash<'d> = Flash<'d, FLASH, Blocking, FLASH_SIZE>;

const FLASH_PAGE_SIZE: usize = PAGE_SIZE;
const FLASH_SECTOR_SIZE: usize = ERASE_SIZE;
const ENTRIES_PER_SECTOR: usize = FLASH_SECTOR_SIZE / FLASH_PAGE_SIZE;
const INVALID_BYTE: u8 = 0xFF;

/// Power state values
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(debug_build, derive(defmt::Format))]
#[repr(u8)]
pub enum PowerState {
    Off = 0,
    On = 1,
}

impl PowerState {
    fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::Off),
            1 => Some(Self::On),
            _ => None,
        }
    }
}

/// Restore mode values
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(debug_build, derive(defmt::Format))]
#[repr(u8)]
pub enum RestoreMode {
    Off = 0,
    On = 1,
    LastState = 2,
}

impl RestoreMode {
    fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::Off),
            1 => Some(Self::On),
            2 => Some(Self::LastState),
            _ => None,
        }
    }
}

/// Flash entry structure (2 bytes)
#[derive(Clone, Copy)]
struct FlashEntry {
    power_state: PowerState,
    restore_mode: RestoreMode,
}

impl Default for FlashEntry {
    fn default() -> Self {
        Self {
            power_state: PowerState::Off,
            restore_mode: RestoreMode::Off,
        }
    }
}

/// Flash store for persistent state
pub struct FlashStore<'d> {
    flash: DcFlash<'d>,
    current_entry: FlashEntry,
}

impl<'d> FlashStore<'d> {
    /// Initialize flash store, reading last valid entry
    pub fn new(flash: DcFlash<'d>) -> Self {
        let mut store = Self {
            flash,
            current_entry: FlashEntry::default(),
        };
        store.current_entry = store.read_last_entry();
        store
    }

    /// Read the last valid entry from flash
    fn read_last_entry(&mut self) -> FlashEntry {
        let mut buf = [0u8; 2];

        // Scan backwards to find the last valid entry
        for i in (0..ENTRIES_PER_SECTOR).rev() {
            let offset = FLASH_TARGET_OFFSET + (i * FLASH_PAGE_SIZE) as u32;

            if self.flash.blocking_read(offset, &mut buf).is_ok()
                && buf[0] != INVALID_BYTE
                && let (Some(power_state), Some(restore_mode)) = (
                    PowerState::from_byte(buf[0]),
                    RestoreMode::from_byte(buf[1]),
                )
            {
                return FlashEntry {
                    power_state,
                    restore_mode,
                };
            }
        }

        // No valid entry found, return default
        FlashEntry::default()
    }

    /// Write an entry to flash.
    ///
    /// Erase/program go through `ram_flash` (direct bootrom sequence); the
    /// embassy-rp blocking flash path hangs on this board's hardware.
    fn write_entry(&mut self, power_state: PowerState, restore_mode: RestoreMode) {
        // Find the next free slot
        let mut entry_index = 0;
        let mut buf = [0u8; 1];

        for i in 0..ENTRIES_PER_SECTOR {
            let offset = FLASH_TARGET_OFFSET + (i * FLASH_PAGE_SIZE) as u32;
            if self.flash.blocking_read(offset, &mut buf).is_ok() && buf[0] == INVALID_BYTE {
                entry_index = i;
                break;
            }
            entry_index = i + 1;
        }

        // Sector full - erase and reset
        if entry_index >= ENTRIES_PER_SECTOR {
            crate::ram_flash::erase(FLASH_TARGET_OFFSET, FLASH_SECTOR_SIZE);
            entry_index = 0;
        }

        // Prepare write buffer (one flash page per entry)
        let mut write_buf = [INVALID_BYTE; FLASH_PAGE_SIZE];
        write_buf[0] = power_state as u8;
        write_buf[1] = restore_mode as u8;

        let offset = FLASH_TARGET_OFFSET + (entry_index * FLASH_PAGE_SIZE) as u32;
        crate::ram_flash::program(offset, &write_buf);
    }

    /// Persist the current in-memory entry to flash.
    fn persist(&mut self) {
        let entry = self.current_entry;
        self.write_entry(entry.power_state, entry.restore_mode);
    }

    /// Get current power state
    pub fn power_state(&self) -> PowerState {
        self.current_entry.power_state
    }

    /// Set power state and persist it to flash (only on an actual change).
    ///
    /// Persistence uses the direct bootrom sequence in `ram_flash` - the
    /// embassy-rp blocking flash path hangs on this board (see ram_flash.rs).
    pub fn set_power_state(&mut self, state: PowerState) {
        if self.current_entry.power_state != state {
            self.current_entry.power_state = state;
            self.persist();
        }
    }

    /// Get current restore mode
    pub fn restore_mode(&self) -> RestoreMode {
        self.current_entry.restore_mode
    }

    /// Set restore mode and persist it to flash (see [`Self::set_power_state`]).
    pub fn set_restore_mode(&mut self, mode: RestoreMode) {
        if self.current_entry.restore_mode != mode {
            self.current_entry.restore_mode = mode;
            self.persist();
        }
    }
}
