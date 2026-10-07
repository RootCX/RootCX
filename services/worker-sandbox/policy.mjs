import { isIP } from 'node:net';

export function allowPublicDestination({ host, port }) {
  try {
    if (typeof host !== 'string' || /[\s/@?#\\%]/.test(host)) return false;
    // URL canonicalization catches decimal, hex and abbreviated IPv4 spellings.
    const hostname = new URL(`https://${host}`).hostname.replace(/^\[|\]$/g, '').replace(/\.$/, '').toLowerCase();
    return [443, 993, 587].includes(port) && !isIP(hostname)
      && hostname !== 'localhost' && !hostname.endsWith('.localhost');
  } catch { return false; }
}

export function workerPolicy({ readPaths, home, storageSocket }) {
  return {
    network: {
      allowedDomains: [], deniedDomains: ['localhost'], strictAllowlist: false,
      // SRT pins DNS answers, but private-use ranges are deliberately opt-in.
      deniedResolvedAddresses: [
        '10.0.0.0/8', '172.16.0.0/12', '192.168.0.0/16', '100.64.0.0/10',
        'fc00::/7', 'fec0::/10', '64:ff9b:1::/48',
      ],
      // Linux cannot allow a single Unix path with seccomp: root masking exposes
      // only the dedicated nonce endpoint, while netns isolates abstract sockets.
      allowAllUnixSockets: true,
    },
    filesystem: {
      denyRead: ['/', '/sys', '/tmp/claude'],
      allowRead: [...new Set([...readPaths, home, storageSocket].filter(Boolean))],
      allowWrite: [home], denyWrite: [],
    },
    enableWeakerNestedSandbox: false,
  };
}
