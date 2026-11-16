//! Tauri IPC command handlers. Names and argument names match `src/types/ipc.ts`
//! (camelCase on the JS side maps to the snake_case Rust parameters).

pub mod backup;
pub mod library;
pub mod lock_screen;
pub mod monitor;
pub mod photo;
pub mod playlist;
pub mod settings;
pub mod steam;
pub mod updater;

pub mod runtime;
