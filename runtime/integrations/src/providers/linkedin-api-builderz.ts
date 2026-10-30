import { IntegrationError } from '../http';
import { action, integer, object, string } from '../schema';
import type { Context, ObjectValue } from '../types';
import { provider } from './shared';

const host = 'realtime-linkedin-bulk-data.p.rapidapi.com';
const helpUrl = 'https://rapidapi.com/apibuilderz/api/realtime-linkedin-bulk-data';
const MAX_PAGE = 10000;
const MAX_TOKEN_LENGTH = 8192;
const pageInput = integer(1, MAX_PAGE);
const paginationToken = string('Opaque nextPaginationToken from the previous profile posts page; send together with nextPage', { maxLength: MAX_TOKEN_LENGTH });
const profileUrl = string('HTTPS LinkedIn profile URL', { pattern: '^https://(www\\.)?linkedin\\.com/in/[^/?#]+/?$', maxLength: 1024 });
const companyUrl = string('HTTPS LinkedIn company URL', { pattern: '^https://(www\\.)?linkedin\\.com/company/[^/?#]+/?$', maxLength: 1024 });
const reactionsUrn = string('Exact reactionsUrn returned by a publications action; it can differ from urn', { pattern: '^urn:li:(activity|share|ugcPost):[0-9]+$', maxLength: 128 });
const commentsUrn = string('Exact commentsUrn returned by a publications action; do not reconstruct it from urn', { pattern: '^urn:li:fsd_socialDetail:\\((urn:li:(?:activity|share|ugcPost):[0-9]+),\\1,urn:li:highlightedReply:-\\)$', maxLength: 512 });

type PageResult = {
  items: ObjectValue[];
  page: number;
  nextPaginationToken: string | null;
  nextPage: number | null;
  hasMore: boolean | null;
  truncated: boolean;
};

function isRecord(value: unknown): value is ObjectValue {
  return !!value && typeof value === 'object' && !Array.isArray(value);
}

function checkedResponse(value: unknown): ObjectValue {
  if (!isRecord(value)) throw new IntegrationError('INVALID_RESPONSE', 'Provider returned an invalid response');
  const failed = value.success !== undefined && value.success !== true;
  const errorStatus = typeof value.status === 'number' && value.status >= 400;
  if (failed || errorStatus || value.error || value.errors) {
    throw new IntegrationError('PROVIDER_REJECTED', 'Provider could not complete the request');
  }
  return value;
}

function linkedinUrl(value: string, kind: 'in' | 'company'): string {
  const parsed = new URL(value);
  if (
    parsed.protocol !== 'https:' || !['linkedin.com', 'www.linkedin.com'].includes(parsed.hostname) ||
    parsed.port || parsed.username || parsed.password ||
    !new RegExp(`^/${kind}/[^/]+/?$`).test(parsed.pathname) || /[%\\\s]/.test(value)
  ) {
    throw new IntegrationError('INVALID_INPUT', 'Use a canonical LinkedIn URL');
  }
  return `https://www.linkedin.com${parsed.pathname.replace(/\/$/, '')}/`;
}

function pageResult(payload: unknown, field: string, page: number, tokenPaging = false): PageResult {
  const data = checkedResponse(payload);
  const items = data[field];
  if (!Array.isArray(items) || items.some(item => !isRecord(item))) {
    throw new IntegrationError('INVALID_RESPONSE', 'Provider omitted the requested items');
  }
  // The observed API sometimes omits pagination. Never report a complete collection from that response.
  let hasMore: boolean | null = null;
  const token = data.paginationToken;
  if (token != null && (typeof token !== 'string' || token.length > MAX_TOKEN_LENGTH)) {
    throw new IntegrationError('INVALID_RESPONSE', 'Provider returned invalid continuation');
  }
  if (token && !tokenPaging) throw new IntegrationError('INVALID_RESPONSE', 'Provider returned unsupported continuation');
  const nextPaginationToken = token || null;
  const pagination = data.pagination;
  if (pagination !== undefined) {
    if (
      !isRecord(pagination) ||
      !['start', 'count', 'total'].every(key => Number.isSafeInteger(pagination[key]) && pagination[key] >= 0) ||
      (pagination.count === 0 && pagination.start < pagination.total)
    ) {
      throw new IntegrationError('INVALID_RESPONSE', 'Provider returned invalid pagination');
    }
    // Profile updates can report total=0 with real items and an opaque continuation token.
    if (pagination.total > 0 || items.length === 0) hasMore = pagination.start + pagination.count < pagination.total;
  }
  if (nextPaginationToken) hasMore = true;
  const canContinue = hasMore !== false && page < MAX_PAGE && (!tokenPaging || !!nextPaginationToken);
  return { items, page, nextPaginationToken, nextPage: canContinue ? page + 1 : null, hasMore, truncated: hasMore !== false && !canContinue };
}

async function listPosts(ctx: Context, kind: 'profile' | 'company', source: string, page: number, paginationToken?: string): Promise<PageResult> {
  const isProfile = kind === 'profile';
  if (isProfile && page > 1 && !paginationToken) {
    throw new IntegrationError('INVALID_INPUT', 'Profile continuation requires the previous nextPaginationToken');
  }
  const response = await ctx.request({
    path: `/${kind}_updates`,
    query: {
      [`${kind}_url`]: linkedinUrl(source, isProfile ? 'in' : 'company'),
      page,
      ...(paginationToken ? { paginationToken } : {}),
    },
  });
  return pageResult(response, 'posts', page, isProfile);
}

export default provider('linkedin_api_builderz', 'LinkedIn · API Builderz', `https://${host}`, {
  list_profile_posts: action(
    'List profile posts',
    'Read one page of profile publications. Continue with BOTH nextPage and nextPaginationToken. Null hasMore means completeness is unknown. Each call may consume quota.',
    'read', { profileUrl, page: pageInput, paginationToken }, ['profileUrl'],
    (ctx, input) => listPosts(ctx, 'profile', input.profileUrl, input.page ?? 1, input.paginationToken),
  ),
  list_company_posts: action(
    'List company posts',
    'Read one page of company publications. Null hasMore means completeness is unknown.',
    'read', { companyUrl, page: pageInput }, ['companyUrl'],
    (ctx, input) => listPosts(ctx, 'company', input.companyUrl, input.page ?? 1),
  ),
  list_post_reactions: action(
    'List post reactions',
    'Read one page of reactions to a publication; no automatic collection or retries.',
    'read', { reactionsUrn, page: pageInput }, ['reactionsUrn'],
    async (ctx, input) => {
      const page = input.page ?? 1;
      const response = await ctx.request({ path: '/post_reactions', query: { reactionsUrn: input.reactionsUrn, page } });
      return pageResult(response, 'reactions', page);
    },
  ),
  list_post_comments: action(
    'List post comments',
    'Read one page of comments to a publication; request subsequent pages explicitly.',
    'read', { commentsUrn, page: pageInput }, ['commentsUrn'],
    async (ctx, input) => {
      const page = input.page ?? 1;
      const response = await ctx.request({ path: '/post_comments', query: { commentsUrn: input.commentsUrn, page } });
      return pageResult(response, 'comments', page);
    },
  ),
  get_person: action(
    'Get person',
    'Read profile details from API Builderz. This POST only reads data; it does not contact the person.',
    'read', { profileUrl }, ['profileUrl'],
    async (ctx, input) => {
      const response = checkedResponse(await ctx.request({ path: '/person', method: 'POST', json: { link: linkedinUrl(input.profileUrl, 'in') } }));
      const person = checkedResponse(response.data ?? response);
      const hasProfileDetails = ['firstName', 'lastName', 'fullName', 'headline', 'position', 'currentCompany'].some(key => person[key] != null);
      if (!hasProfileDetails) throw new IntegrationError('INVALID_RESPONSE', 'Provider omitted profile details');
      return { person };
    },
  ),
}, {
  version: '0.1.0',
  description: 'Read LinkedIn publications, reactions, comments and profiles through API Builderz on RapidAPI. Independent of LinkedIn; requires your own subscription and key.',
  requestsPerSecond: 1,
  credentialSchema: object({ accessToken: string('RapidAPI key', { maxLength: 512, pattern: '^\\S+$' }) }, ['accessToken']),
  tokenAuth: { label: 'RapidAPI key', helpUrl },
  authorize: credential => ({ 'x-rapidapi-key': credential.accessToken, 'x-rapidapi-host': host }),
  async verify(ctx) {
    await listPosts(ctx, 'company', 'https://www.linkedin.com/company/linkedin/', 1);
    // One key slot per user/provider: reconnecting rotates it. This proves API access, not LinkedIn identity.
    return { id: 'rapidapi', label: 'API Builderz' };
  },
});
