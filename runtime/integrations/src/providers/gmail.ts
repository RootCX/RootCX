import { provider, googleOAuth, googleServices } from './shared';
import { action, array, id, pagination, page, segment, string } from '../schema';
import type { ObjectValue } from '../types';

const mail = {
  to: { ...array(string('Bare email address', { maxLength: 254, pattern: '^[^\\s<>@,;]+@[^\\s<>@,;]+\\.[^\\s<>@,;]+$' }), 50), minItems: 1 },
  subject: string('Subject', { maxLength: 998, pattern: '^[^\\r\\n]*$' }),
  text: string('Plain text body', { minLength: 0, maxLength: 100000 }),
};
function message(input: ObjectValue) {
  const body = Buffer.from(input.text, 'utf8').toString('base64').match(/.{1,76}/g)?.join('\r\n') ?? '';
  // Fold encoded words without splitting UTF-8 characters (RFC 2047: 75 octets).
  const words: string[] = []; let chunk = '';
  for (const char of input.subject) {
    if (Buffer.byteLength(chunk + char) > 42) { words.push(chunk); chunk = ''; }
    chunk += char;
  }
  if (chunk) words.push(chunk);
  const subject = words.map(word => `=?UTF-8?B?${Buffer.from(word).toString('base64')}?=`).join('\r\n ');
  return { raw: Buffer.from(`To: ${input.to.join(',\r\n ')}\r\nSubject: ${subject}\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=UTF-8\r\nContent-Transfer-Encoding: base64\r\n\r\n${body}\r\n`).toString('base64url') };
}
export default provider('gmail', 'Gmail', 'https://www.googleapis.com', {
  search_messages: action('Search messages', 'Search Gmail using native q syntax', 'read', { ...pagination, query: string() }, [], async (ctx, input) => {
    const result = await ctx.request({ path: '/gmail/v1/users/me/messages', query: { q: input.query, maxResults: input.limit ?? 50, pageToken: input.cursor } });
    return page(result.messages ?? [], result.nextPageToken);
  }),
  get_message: action('Get message', 'Read a message including headers and MIME payload', 'read', { messageId: id() }, ['messageId'], (ctx, input) => ctx.request({ path: `/gmail/v1/users/me/messages/${segment(input.messageId)}`, query: { format: 'full' } })),
  create_draft: action('Create draft', 'Create a plain-text email draft. Does not send it.', 'write', mail, ['to', 'subject', 'text'], (ctx, input) => ctx.request({ path: '/gmail/v1/users/me/drafts', method: 'POST', json: { message: message(input) } })),
  send_message: action('Send message', 'Send a plain-text email. Never automatically retry an uncertain send.', 'write', mail, ['to', 'subject', 'text'], (ctx, input) => ctx.request({ path: '/gmail/v1/users/me/messages/send', method: 'POST', json: message(input) })),
}, { services: googleServices, oauth: googleOAuth(['https://www.googleapis.com/auth/gmail.readonly', 'https://www.googleapis.com/auth/gmail.compose']) });
