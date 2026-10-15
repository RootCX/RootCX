import { provider, slackRequest } from './shared';
import { action, id, integer, pagination, page, string } from '../schema';
import type { Context } from '../types';

const verify = async (ctx: Context) => {
  const account = await slackRequest(ctx, 'auth.test');
  if (!account.team_id || !account.user_id) throw new Error('Slack account identifier missing');
  return { id: `${account.team_id}:${account.user_id}`, label: `${account.team} / ${account.user}` };
};
export default provider('slack', 'Slack', 'https://slack.com', {
  list_channels: action('List channels', 'List public and private channels accessible to this bot', 'read', pagination, [], async (ctx, input) => {
    const result = await slackRequest(ctx, 'conversations.list', { limit: input.limit ?? 50, cursor: input.cursor, types: 'public_channel,private_channel', exclude_archived: true });
    return page(result.channels, result.response_metadata?.next_cursor);
  }),
  get_history: action('Read channel history', 'Read up to 15 messages; non-Marketplace commercial apps may be limited to one request per minute', 'read', { channelId: id(), ...pagination, limit: integer(1, 15) }, ['channelId'], async (ctx, input) => {
    const result = await slackRequest(ctx, 'conversations.history', { channel: input.channelId, limit: input.limit ?? 15, cursor: input.cursor });
    return page(result.messages, result.response_metadata?.next_cursor);
  }),
  send_message: action('Send channel message', 'Post a bot message. Never automatically retry an uncertain send.', 'write', { channelId: id(), text: string('Slack mrkdwn text', { maxLength: 4000 }), threadTs: string('Parent message timestamp', { pattern: '^\\d+\\.\\d+$' }) }, ['channelId', 'text'], (ctx, input) => slackRequest(ctx, 'chat.postMessage', { channel: input.channelId, text: input.text, thread_ts: input.threadTs, unfurl_links: false, unfurl_media: false }, 'write')),
}, { verify, tokenAuth: { label: 'Slack bot access token', helpUrl: 'https://api.slack.com/apps' },
  oauth: { authorizeUrl: 'https://slack.com/oauth/v2/authorize', tokenUrl: 'https://slack.com/api/oauth.v2.access', scopes: ['channels:read', 'groups:read', 'channels:history', 'groups:history', 'chat:write'], scopeSeparator: ',', account: async (_token, ctx) => verify(ctx) },
  discovery: { action: 'list_channels', itemsPath: 'items', idPath: 'id', labelPath: 'name' },
});
