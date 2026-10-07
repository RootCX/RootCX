import { join } from 'node:path';

export function sandboxPolicy({ root, home, readPaths, providerUrl }) {
  const allowedDomains = ['registry.npmjs.org:443'];
  if (providerUrl) {
    const endpoint = new URL(providerUrl);
    if (endpoint.protocol !== 'http:' || endpoint.hostname !== '127.0.0.1' || !endpoint.port || endpoint.username || endpoint.password) {
      throw new Error('Provider bridge must be an explicit loopback port');
    }
    allowedDomains.push(`127.0.0.1:${endpoint.port}`);
  }
  return {
    network: {
      allowedDomains, deniedDomains: [], strictAllowlist: true,
      deniedResolvedAddresses: ['10.0.0.0/8', '172.16.0.0/12', '192.168.0.0/16', '100.64.0.0/10', 'fc00::/7'],
      allowAllUnixSockets: false,
    },
    filesystem: {
      denyRead: ['/', '/sys', '/tmp/claude'], allowRead: readPaths,
      allowWrite: [root, home],
      denyWrite: ['.git', join(home, '.agents'), join(home, '.claude'), join(home, '.config')],
    },
    enableWeakerNestedSandbox: false,
  };
}
