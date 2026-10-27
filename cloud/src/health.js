/// Health rows the Worker stores. Address lists used to be dropped here, so
/// the other server never learned them and clients never received them.

function addressList(value) {
  if (!Array.isArray(value)) return [];
  return value
    .map((item) => String(item || "").trim())
    .filter(Boolean)
    .slice(0, 8);
}

export function shapeHealth(body, at) {
  const src = body && typeof body === "object" ? body : {};
  return {
    instance: src.instance,
    name: String(src.name || "").slice(0, 64),
    version: String(src.version || "").slice(0, 32),
    role: src.role === "active" ? "active" : "standby",
    epoch: Number.isFinite(src.epoch) ? src.epoch : 0,
    healthy: !!src.healthy,
    at,
    maintenance_until: Number.isFinite(src.maintenance_until) ? src.maintenance_until : 0,
    public_addrs: addressList(src.public_addrs),
    lan_addrs: addressList(src.lan_addrs),
    names: addressList(src.names),
  };
}

/// A public IPv4 and the signaling port. Private addresses are refused so
/// this route cannot be used to scan a local network.
export function probeQuery(host, port) {
  const text = String(host || "").trim();
  const p = Number(port);
  if (!/^(?:\d{1,3}\.){3}\d{1,3}$/.test(text) || p !== 21200) return null;
  const parts = text.split(".").map(Number);
  if (parts.some((n) => n > 255)) return null;
  const [a, b] = parts;
  if (a === 0 || a === 10 || a === 127) return null;
  if (a === 192 && b === 168) return null;
  if (a === 172 && b >= 16 && b <= 31) return null;
  if (a === 100 && b >= 64 && b <= 127) return null;
  return { host: text, port: p };
}
