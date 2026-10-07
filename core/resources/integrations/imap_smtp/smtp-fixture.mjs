import { createServer } from 'node:net';
import { createSecureContext, TLSSocket } from 'node:tls';
import { readFileSync } from 'node:fs';

const secureContext = createSecureContext({ cert: readFileSync(process.argv[2]), key: readFileSync(process.argv[3]) });
function protocol(stream, secured) {
  let buffer = '', body = false;
  stream.on('error', () => {});
  const onData = data => {
    buffer += data.toString();
    while (buffer.includes('\r\n')) {
      const end = buffer.indexOf('\r\n'), line = buffer.slice(0, end); buffer = buffer.slice(end + 2);
      if (body) {
        if (line === '.') { body = false; process.send({ delivered: true }); stream.write('250 queued\r\n'); }
        continue;
      }
      if (line.startsWith('EHLO')) stream.write(secured ? '250-test\r\n250 AUTH PLAIN\r\n' : '250-test\r\n250 STARTTLS\r\n');
      else if (line === 'STARTTLS') {
        stream.removeListener('data', onData); stream.write('220 upgrade\r\n');
        protocol(new TLSSocket(stream, { isServer: true, secureContext }), true); return;
      } else if (line.startsWith('AUTH')) stream.write('235 authenticated\r\n');
      else if (line === 'DATA') { body = true; stream.write('354 message\r\n'); }
      else if (line === 'QUIT') { stream.end('221 bye\r\n'); return; }
      else stream.write('250 ok\r\n');
    }
  };
  stream.on('data', onData);
}
const server = createServer(socket => { socket.write('220 test SMTP\r\n'); protocol(socket, false); });
server.listen(0, '127.0.0.1', () => process.send({ port: server.address().port }));
process.on('disconnect', () => process.exit());
