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

#![allow(dead_code)]

// ── New architecture (ZeroTrace) ─────────────────────────────────────
pub mod bundles;
pub mod collectors;
pub mod extensions;
pub mod processors;
pub mod reporters;

// ── Shared modules (used by both old and new code) ───────────────────
pub mod common;
pub mod config;
pub mod debug;
mod error;
pub mod exception;
pub mod utils;

// ── Legacy DeepFlow modules (to be removed by M4) ────────────────────
pub mod legacy;

// Re-exports: keep old crate:: paths working during migration.
// Remove these when deleting src/legacy/ at M4.
// Re-export legacy ebpf at the old crate::ebpf path
#[cfg(all(unix, feature = "libtrace"))]
pub use crate::collectors::ebpf::legacy as ebpf;
#[cfg(all(unix, feature = "libtrace"))]
pub use crate::legacy::ebpf_dispatcher;
pub use crate::legacy::{
    collector, dispatcher, flow_generator, handler, integration_collector, metric, monitor,
    platform, plugin, policy, rpc, sender, trident,
};
// for benchmarks
#[doc(hidden)]
pub use {
    common::{
        Timestamp as _Timestamp,
        endpoint::{
            EndpointData as _EndpointData, EndpointInfo as _EndpointInfo,
            FeatureFlags as _FeatureFlags,
        },
        enums::TcpFlags as _TcpFlags,
        feature as _feature,
        flow::PacketDirection as _PacketDirection,
        l7_protocol_log::L7PerfCache as _L7PerfCache,
        l7_protocol_log::{LogCache as _LogCache, LogCacheKey as _LogCacheKey},
        lookup_key::LookupKey as _LookupKey,
        platform_data::{IpSubnet as _IpSubnet, PlatformData as _PlatformData},
        policy::{
            Acl as _Acl, Cidr as _Cidr, Container as _Container, IpGroupData as _IpGroupData,
        },
        port_range::PortRange as _PortRange,
    },
    legacy::flow_generator::HttpLog,
    legacy::flow_generator::flow_map::{
        _new_flow_map_and_receiver, _new_meta_packet, _reverse_meta_packet,
        Config as _FlowMapConfig,
    },
    legacy::flow_generator::perf::{
        FlowPerfCounter as _FlowPerfCounter, L7FlowPerf as _L7FlowPerf,
        tcp::{
            _benchmark_report, _benchmark_session_peer_seq_no_assert, _meta_flow_perf_update,
            TcpPerf as _TcpPerf,
        },
    },
    legacy::policy::fast_path::EndpointTableType as _EndpointTableType,
    legacy::policy::first_path::FirstPath as _FirstPath,
    legacy::policy::labeler::Labeler as _Labeler,
    npb_pcap_policy::{
        DirectionType as _DirectionType, NpbAction as _NpbAction, NpbTunnelType as _NpbTunnelType,
        TapSide as _TapSide,
    },
};

#[allow(unused)]
macro_rules! gen_sizes {
    ($(#[$struct_meta:meta])*
    $sv:vis struct $name:ident {
        $($(#[$field_meta:meta])* $fv:vis $fname:ident: $ftype:ty),* $(,)?
    }
    ) => {
        $(#[$struct_meta])*
        $sv struct $name {
            $($(#[$field_meta])* $fv $fname: $ftype,)*
        }

        impl $name {
            pub fn print_sizes() {
                println!("{}\t{} struct {{", std::mem::size_of::<$name>(), stringify!($name));
                $(println!("{}\t\t{}: {},", std::mem::size_of::<$ftype>(), stringify!($fname), stringify!($ftype));)*
                println!("\t}}");
            }
        }
    };

    ($(#[$struct_meta:meta])*
    $sv:vis struct $name:ident (
        $($(#[$field_meta:meta])* $fv:vis $ftype:ty),*
    );
    ) => {
        $(#[$struct_meta])*
        $sv struct $name ($($(#[$field_meta])* $fv $ftype),*);

        impl $name {
            pub fn print_sizes() {
                println!("{}\t{} struct {{", std::mem::size_of::<$name>(), stringify!($name));
                $(println!("{}\t\t{},", std::mem::size_of::<$ftype>(), stringify!($ftype));)*
                println!("\t}}");
            }
        }
    };

    ($(#[$struct_meta:meta])*
    $sv:vis enum $name:ident {
        $($(#[$field_meta:meta])* $fname:ident($ftype:ty)),* $(,)?
    }
    ) => {
        $(#[$struct_meta])*
        $sv enum $name {
            $($(#[$field_meta])* $fname($ftype),)*
        }

        impl $name {
            pub fn print_sizes() {
                println!("{}\t{} enum {{", std::mem::size_of::<$name>(), stringify!($name));
                $(println!("{}\t\t{}: {},", std::mem::size_of::<$ftype>(), stringify!($fname), stringify!($ftype));)*
                println!("\t}}");
            }
        }
    };
}

#[allow(unused)]
pub(crate) use gen_sizes;

#[cfg(test)]
mod tests {
    macro_rules! print_size_of {
        ($(($spaces: expr, $t: ty)),*) => {
            $({
                println!(concat!($spaces, stringify!($t), ": {}"), std::mem::size_of::<$t>());
            })*
        };
    }

    #[test]
    fn struct_sizes() {
        #[rustfmt::skip]
        print_size_of![
            ("", crate::legacy::flow_generator::flow_node::FlowNode),
            ("    ", crate::common::TaggedFlow),
            ("        ", crate::common::flow::Flow),
            ("            ", crate::common::flow::FlowKey),
            ("         2x ", crate::common::flow::FlowMetricsPeer),
            ("            ", crate::common::flow::TunnelField),
            ("         -> ", crate::common::flow::FlowPerfStats),
            ("        ", crate::common::tag::Tag),
            ("    ", crate::legacy::flow_generator::flow_state::FlowState),
            (" -> ", crate::legacy::flow_generator::perf::FlowLog),
            ("        ", crate::legacy::flow_generator::perf::L4FlowPerfTable),
            ("         +> ", crate::legacy::flow_generator::perf::tcp::TcpPerf),
            ("         |      ", crate::legacy::flow_generator::perf::tcp::PerfControl),
            ("         |       2x ", crate::legacy::flow_generator::perf::tcp::SessionPeer),
            ("         |      ", crate::legacy::flow_generator::perf::tcp::PerfData),
            ("         -- ", crate::legacy::flow_generator::perf::udp::UdpPerf),
            ("         +> ", crate::legacy::flow_generator::protocol_logs::sql::PostgresqlLog),
            ("         +> ", crate::legacy::flow_generator::protocol_logs::rpc::SofaRpcLog),
            ("     -> ", crate::common::l7_protocol_log::L7ProtocolParser),
            ("         +- ", crate::legacy::flow_generator::protocol_logs::http::HttpLog),
            ("         +- ", crate::legacy::flow_generator::protocol_logs::dns::DnsLog),
            ("         +- ", crate::legacy::flow_generator::protocol_logs::rpc::SofaRpcLog),
            ("         +- ", crate::legacy::flow_generator::protocol_logs::sql::MysqlLog),
            ("         +- ", crate::legacy::flow_generator::protocol_logs::mq::KafkaLog),
            ("         +- ", crate::legacy::flow_generator::protocol_logs::sql::RedisLog),
            ("         +- ", crate::legacy::flow_generator::protocol_logs::sql::PostgresqlLog),
            ("         +- ", crate::legacy::flow_generator::protocol_logs::rpc::DubboLog),
            ("         +- ", crate::legacy::flow_generator::protocol_logs::mq::MqttLog),
            (" 2x ", npb_pcap_policy::PolicyData),
            (" 2x ", crate::common::endpoint::EndpointData),
            (" -> ", packet_sequence_block::PacketSequenceBlock)
        ];
    }
}
