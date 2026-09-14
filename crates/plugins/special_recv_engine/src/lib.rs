/*
 * Copyright (c) 2024 Yunshan Networks
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Enterprise Edition Feature: windows-dispatcher

use public::{
    counter,
    debug::QueueDebugger,
    error::{Error, Result},
    packet,
    queue::Receiver,
};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

/// Counters for all live libpcap handles owned by one dispatcher.
#[derive(Default)]
pub struct LibpcapCounter {
    packets: AtomicU64,
    drops: AtomicU64,
    errors: AtomicU64,
}

impl counter::RefCountable for LibpcapCounter {
    fn get_counters(&self) -> Vec<counter::Counter> {
        vec![
            (
                "kernel_packets",
                counter::CounterType::Counted,
                counter::CounterValue::Unsigned(self.packets.load(Ordering::Relaxed)),
            ),
            (
                "kernel_drops",
                counter::CounterType::Counted,
                counter::CounterValue::Unsigned(self.drops.load(Ordering::Relaxed)),
            ),
            (
                "poll_error",
                counter::CounterType::Counted,
                counter::CounterValue::Unsigned(self.errors.load(Ordering::Relaxed)),
            ),
        ]
    }
}

struct CaptureHandle {
    capture: pcap::Capture<pcap::Active>,
    if_index: isize,
}

pub struct Libpcap {
    captures: Vec<CaptureHandle>,
    next_capture: usize,
    counter: Arc<LibpcapCounter>,
}

impl Libpcap {
    /// Open one libpcap handle per interface. Windows pcap device names are
    /// GUID-like strings (for example `\\Device\\NPF_{...}`), so callers must
    /// pass the name returned by the Windows interface enumeration unchanged.
    pub fn new(
        interfaces: Vec<(&str, isize)>,
        block_size: usize,
        snap_len: usize,
        _: &QueueDebugger,
    ) -> Result<Self> {
        if interfaces.is_empty() {
            return Err(Error::LibpcapError("no capture interface specified".into()));
        }

        let mut captures = Vec::with_capacity(interfaces.len());
        for (name, if_index) in interfaces {
            let snap_len = i32::try_from(snap_len)
                .map_err(|_| Error::LibpcapError("snap length is too large".into()))?;
            let buffer_size = block_size
                .checked_mul(1024 * 1024)
                .and_then(|size| i32::try_from(size).ok())
                .ok_or_else(|| Error::LibpcapError("pcap buffer size is too large".into()))?;
            let capture = pcap::Capture::from_device(name)
                .map_err(|e| Error::LibpcapError(format!("open device {name}: {e}")))?
                .promisc(true)
                .snaplen(snap_len)
                // A finite timeout is important when more than one interface
                // is configured; it lets read() poll the other handles.
                .timeout(100)
                // Npcap/WinPcap otherwise waits for min-to-copy/buffer batching,
                // which makes low-rate traffic appear delayed or missing.
                .immediate_mode(true)
                .buffer_size(buffer_size)
                .open()
                .map_err(|e| Error::LibpcapError(format!("activate device {name}: {e}")))?
                // A read timeout is not reliable for every Npcap/WinPcap mode.
                // Without non-blocking mode, an idle handle can block here and
                // starve all interfaces that follow it in the round-robin loop.
                .setnonblock()
                .map_err(|e| {
                    Error::LibpcapError(format!("set device {name} non-blocking: {e}"))
                })?;
            captures.push(CaptureHandle { capture, if_index });
        }

        Ok(Self {
            captures,
            next_capture: 0,
            counter: Arc::new(LibpcapCounter::default()),
        })
    }

    /// Read one packet and copy it out of libpcap's reusable receive buffer.
    /// The copy is required because pcap invalidates the returned slice on the
    /// next call to `next()`.
    pub unsafe fn read(&mut self) -> Result<packet::Packet<'_>> {
        let count = self.captures.len();
        if count == 0 {
            return Err(Error::LibpcapError("all capture handles are closed".into()));
        }

        for _ in 0..count {
            let index = self.next_capture % count;
            self.next_capture = (index + 1) % count;
            let handle = &mut self.captures[index];
            match handle.capture.next() {
                Ok(captured) => {
                    let timestamp = timeval_to_duration(captured.header.ts)
                        .ok_or_else(|| Error::LibpcapError("invalid packet timestamp".into()))?;
                    let mut data = captured.data.to_vec();
                    let ptr = data.as_mut_ptr();
                    let length = data.len();
                    std::mem::forget(data);
                    self.counter.packets.fetch_add(1, Ordering::Relaxed);
                    return Ok(packet::Packet {
                        timestamp,
                        if_index: handle.if_index,
                        capture_length: captured.header.caplen as isize,
                        data: std::slice::from_raw_parts_mut(ptr, length),
                        raw: Some(ptr),
                    });
                },
                Err(pcap::Error::TimeoutExpired) => continue,
                Err(err) => {
                    self.counter.errors.fetch_add(1, Ordering::Relaxed);
                    return Err(Error::LibpcapError(err.to_string()));
                },
            }
        }
        // All handles are non-blocking. Yield briefly when none has a packet
        // so the dispatcher does not spin at 100% CPU on an idle machine.
        std::thread::sleep(Duration::from_millis(1));
        Err(Error::Timeout)
    }

    pub fn set_bpf(&mut self, filter: &str) -> Result<()> {
        for handle in &mut self.captures {
            handle
                .capture
                .filter(filter, true)
                .map_err(|e| Error::LibpcapError(format!("set BPF filter: {e}")))?;
        }
        Ok(())
    }

    pub fn get_counter_handle(&self) -> Arc<dyn counter::RefCountable> {
        self.counter.clone()
    }
}

fn timeval_to_duration(tv: libc::timeval) -> Option<Duration> {
    if tv.tv_sec < 0 || tv.tv_usec < 0 {
        return None;
    }
    Some(Duration::new(
        tv.tv_sec as u64,
        (tv.tv_usec as u32).saturating_mul(1_000),
    ))
}

pub struct Dpdk;

impl Dpdk {
    pub fn new(_: Option<String>, _: Option<String>, _: usize) -> Self {
        unimplemented!();
    }

    pub unsafe fn read(&mut self) -> Result<packet::Packet<'_>> {
        unimplemented!();
    }

    pub fn get_counter_handle(&self) -> Arc<dyn counter::RefCountable> {
        unimplemented!();
    }
}

pub struct VhostUser;

impl VhostUser {
    pub fn new(_: String, _: usize) -> Self {
        unimplemented!();
    }

    pub unsafe fn read(&mut self) -> Result<packet::Packet<'_>> {
        unimplemented!();
    }

    pub fn get_counter_handle(&self) -> Arc<dyn counter::RefCountable> {
        unimplemented!();
    }
}

pub struct DpdkFromEbpf;

impl DpdkFromEbpf {
    pub fn new(_: Receiver<Box<packet::Packet<'static>>>, _: Duration) -> Self {
        unimplemented!();
    }

    pub unsafe fn read(&mut self) -> Result<Box<packet::Packet<'_>>> {
        unimplemented!();
    }

    pub fn get_counter_handle(&self) -> Arc<dyn counter::RefCountable> {
        unimplemented!();
    }
}
