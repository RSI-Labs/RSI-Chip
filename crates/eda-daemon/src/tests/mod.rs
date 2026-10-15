use crate::jobs::{write_remote_job_record, WorkerCommand};
use crate::*;
use std::fs;
use std::io::{Cursor, Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

mod ams;
mod core;
mod jobs;
mod layout;
mod rtl;
mod server;
