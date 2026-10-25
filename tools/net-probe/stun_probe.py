"""Isolated NAT mapping probe: one UDP socket, STUN Binding requests (RFC 5389)
to several public servers. Same mapped port for every destination suggests
endpoint-independent mapping (hole-punch friendly); different ports suggest
endpoint-dependent ("symmetric") mapping. Sends no user data."""
import os, socket, struct, sys, json, time
SERVERS = [("stun.l.google.com", 19302), ("stun1.l.google.com", 19302), ("stun.cloudflare.com", 3478)]
MAGIC = 0x2112A442
def binding(sock, host, port):
    txid = os.urandom(12)
    sock.sendto(struct.pack("!HHI12s", 0x0001, 0, MAGIC, txid), (host, port))
    sock.settimeout(2.0)
    while True:
        data, src = sock.recvfrom(2048)
        if data[8:20] != txid: continue
        pos = 20
        while pos + 4 <= len(data):
            atype, alen = struct.unpack("!HH", data[pos:pos+4]); val = data[pos+4:pos+4+alen]
            if atype == 0x0020 and val[1] == 0x01:  # XOR-MAPPED-ADDRESS, IPv4
                xport = struct.unpack("!H", val[2:4])[0] ^ (MAGIC >> 16)
                xip = struct.unpack("!I", val[4:8])[0] ^ MAGIC
                return socket.inet_ntoa(struct.pack("!I", xip)), xport, src
            pos += 4 + alen + ((4 - alen % 4) % 4)
        return None
results = []
sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); sock.bind(("0.0.0.0", 0))
local_port = sock.getsockname()[1]
for host, port in SERVERS:
    try:
        ip = socket.gethostbyname(host)
        mapped = binding(sock, ip, port)
        results.append({"server": host, "server_ip": ip, "mapped": None if not mapped else f"{mapped[0]}:{mapped[1]}"})
    except Exception as e:
        results.append({"server": host, "error": type(e).__name__})
ports = {r["mapped"].split(":")[1] for r in results if r.get("mapped")}
ips = {r["mapped"].split(":")[0] for r in results if r.get("mapped")}
print(json.dumps({"local_port": local_port, "results": results,
  "mapped_public_ips": len(ips), "distinct_mapped_ports": len(ports),
  "port_preserved": bool(ports) and ports == {str(local_port)},
  "inference": "no STUN response (UDP blocked?)" if not ports else ("endpoint-independent mapping (EIM)" if len(ports) == 1 else "endpoint-dependent mapping (symmetric-like)")}, indent=1))
