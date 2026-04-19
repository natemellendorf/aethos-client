# Bonjour/mDNS Service Discovery Specification

Version: 1.0
Status: Final
Date: 2026-04-19

## 1. Overview

Aethos uses mDNS/DNS-SD (Bonjour) for zero-configuration Local Area Network (LAN) peer discovery. This mechanism allows peers to find each other on the same network segment without manual IP configuration. Peers advertise a UDP gossip service and browse for other peers on the local network. This discovery process is used to locate peers for gossip synchronization (utilizing the HELLO, SUMMARY, REQUEST, and TRANSFER protocol).

## 2. Terminology

The key words "MUST", "MUST NOT", "REQUIRED", "SHALL", "SHALL NOT", "SHOULD", "SHOULD NOT", "RECOMMENDED", "MAY", and "OPTIONAL" in this document are to be interpreted as described in RFC 2119.

- **Peer**: An instance of an Aethos client running on a local network.
- **Instance**: A specific named occurrence of a service advertised via mDNS.
- **Service Type**: The DNS-SD service identifier used to categorize the application.
- **TXT Record**: A DNS resource record used to provide additional metadata about a service.
- **Endpoint**: A network address (IP and port) used for peer-to-peer communication.
- **Gossip Sync**: The protocol used by Aethos peers to synchronize state over UDP.

## 3. Service Advertisement

### 3.1 Service Type

The service type MUST be `_aethos._udp.local.`. The domain MUST be `local.`. This is defined as a UDP service type because the gossip synchronization protocol utilizes UDP for transport.

### 3.2 Port

The advertised port MUST be the gossip LAN port used for UDP gossip sync. The default gossip LAN port is `47655`. Implementations MAY allow the port to be overridden via configuration or the environment variable `AETHOS_GOSSIP_LAN_PORT`.

### 3.3 TXT Records

Implementations MUST include exactly two TXT record key-value pairs:

- `txtvers` = `1` (The TXT record schema version).
- `api` = `gossipv1` (The protocol API version).

Implementations MUST NOT include peer identity information (such as wayfarer_id or keys) in TXT records. Future TXT record versions MUST increment the `txtvers` value. Receivers SHOULD ignore unknown TXT keys to maintain forward compatibility.

### 3.4 Instance Name

The instance name SHOULD be derived from the local hostname suffixed with ` Aethos` (e.g., `myhost Aethos`). If the hostname is unavailable, the instance name SHOULD default to `Aethos Desktop` or a platform-appropriate equivalent such as `Aethos iOS`.

The instance name MUST NOT exceed 63 bytes when UTF-8 encoded. It MUST be truncated at a character boundary if necessary. The instance name is a human-readable display label only. Implementations MUST NOT use it for peer identification or authentication.

### 3.5 Hostname

The DNS hostname used in the SRV record MUST be a valid DNS label suffixed with `.local.`. The hostname label MUST be derived by performing these steps:

1. Read the system hostname using platform-specific methods (e.g., `HOSTNAME` environment variable, `/etc/hostname`, or `gethostname()`).
2. Replace any character that is not ASCII alphanumeric or a hyphen (`-`) with a hyphen.
3. Convert all characters to ASCII lowercase.
4. Trim any leading or trailing hyphens.
5. Truncate the result to a maximum of 63 bytes.

If the sanitized result is empty, the hostname label MUST default to `aethos-desktop` or a platform-appropriate equivalent.

### 3.6 Address Auto-Population

Implementations SHOULD use the mDNS library's automatic address population features rather than manually specifying interface addresses. This ensures the service is advertised on all active and relevant network interfaces.

## 4. Service Browsing

### 4.1 Browse Target

Implementations MUST browse for the service type `_aethos._udp.local.`.

### 4.2 Event Processing

When a service is resolved, implementations MUST extract the service fullname (the DNS-SD full service name) and the resolved IPv4 address(es) and port.

Implementations MUST select the first available IPv4 address from the resolved service for endpoint construction. Implementations SHOULD emit a `PeerDiscovered` event when a service is resolved, occurring before endpoint selection.

Implementations MUST emit an `EndpointResolved` event only when at least one IPv4 address is available. If no IPv4 address is present in the resolved service, implementations MUST NOT emit an `EndpointResolved` event. Implementations MAY additionally resolve IPv6 addresses but MUST support IPv4 as the baseline.

### 4.3 Browse Failure

If the browse operation stops unexpectedly, implementations MUST surface this as an error event. Browse failures MUST NOT cause the application to crash. They SHOULD be logged and reported through the appropriate application channels.

## 5. Disable / Enable Controls

### 5.1 Environment Variables

Implementations MUST support disabling Bonjour discovery via either of two environment variables:

- `AETHOS_DISABLE_BONJOUR`
- `AETHOS_DISABLE_MDNS`

When either variable is set to `1` or `true` (case-insensitive), Bonjour MUST be fully disabled. This means no advertisement and no browsing occurs. When disabled, the discovery component MUST enter a no-op state and MUST NOT attempt daemon initialization.

## 6. Error Handling

### 6.1 Initialization Failures

If mDNS daemon initialization, browse registration, service info creation, or service registration fails, the implementation MUST fall back to a disabled or no-op state. The error MUST be surfaced as a discovery error event rather than a crash or panic. Application operation MUST continue without Bonjour functionality.

### 6.2 Runtime Failures

Events indicating that a browse has stopped MUST be surfaced as error events. Implementations MUST NOT retry or restart the mDNS daemon automatically on failure. A new discovery session MAY be created on the next appropriate application lifecycle event.

## 7. Security Considerations

Peer identity (such as wayfarer_id or signing keys) MUST NOT be exposed in mDNS advertisements, including TXT records or instance names.

As mDNS operates on the local network segment only, implementations MUST NOT assume discovery results are authenticated. Gossip synchronization initiated after discovery MUST use its own authentication and verification mechanisms, such as a HELLO frame with a signed identity.

## 8. Platform Implementation Notes

- **Rust (reference)**: Uses the `mdns-sd` crate. Typical flow involves `ServiceDaemon::new()`, `.browse()`, and `ServiceInfo::new().enable_addr_auto().register()`.
- **Apple (iOS/macOS)**: MAY use `NWBrowser` and `NWListener` from Network.framework or the older `NetService` and `NSNetServiceBrowser` APIs. Note that the service type string for Apple APIs MUST be `_aethos._udp.` without the trailing `local.`, as Apple APIs append the domain automatically.
- **Android**: MAY use `NsdManager` for DNS-SD discovery.
- **Windows**: MAY use native Windows DNS-SD APIs or a cross-platform mDNS library.

All platforms MUST conform to the service type, TXT record schema, port, and security requirements defined in this specification.

## 9. Conformance Checklist

| Requirement | Section | Priority |
|---|---|---|
| Service type is `_aethos._udp.local.` | 3.1 | MUST |
| Port defaults to 47655 | 3.2 | MUST |
| TXT includes `txtvers=1` | 3.3 | MUST |
| TXT includes `api=gossipv1` | 3.3 | MUST |
| TXT excludes peer identity | 3.3 | MUST NOT |
| Instance name ≤ 63 bytes | 3.4 | MUST |
| Hostname label is DNS-safe | 3.5 | MUST |
| Browse for `_aethos._udp.local.` | 4.1 | MUST |
| Select first IPv4 from resolved | 4.2 | MUST |
| Supports AETHOS_DISABLE_BONJOUR/MDNS | 5.1 | MUST |
| Initialization failure → graceful fallback | 6.1 | MUST |
| No peer identity in advertisements | 7 | MUST NOT |

## 10. Version History

- v1.0 — Initial specification (derived from aethos-client reference implementation, commit range eda5dcc..19bd51e)
