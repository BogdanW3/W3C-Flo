pub use crate::proto::flo_observer::*;

packet_type!(ObserverConnect, PacketObserverConnect);
packet_type!(ObserverConnectAccept, PacketObserverConnectAccept);
packet_type!(ObserverConnectReject, PacketObserverConnectReject);
packet_type!(ObserverPasswordRequest, PacketObserverPasswordRequest);
packet_type!(ObserverPasswordResponse, PacketObserverPasswordResponse);