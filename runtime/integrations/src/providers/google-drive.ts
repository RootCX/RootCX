import { provider, googleOAuth, googleServices } from './shared';
import { action, boolean, id, pagination, page, segment, string } from '../schema';

const oauth = googleOAuth(['https://www.googleapis.com/auth/drive']);
const account = oauth.account;
oauth.authorizationParams = { ...oauth.authorizationParams, prompt: 'select_account consent' };
oauth.account = async (token, ctx) => {
  const about = await ctx.request({ path: '/drive/v3/about', query: { fields: 'user(permissionId)' } });
  if (!about.user?.permissionId) throw new Error('Invalid Google Drive response');
  return account(token, ctx);
};

export default provider('google_drive', 'Google Drive', 'https://www.googleapis.com', {
  list_files: action('List files', 'Search accessible Drive files using the native q expression', 'read', { ...pagination, query: string() }, [], async (ctx, input) => {
    const result = await ctx.request({ path: '/drive/v3/files', query: { q: input.query ?? 'trashed = false', pageSize: input.limit ?? 50, pageToken: input.cursor, fields: 'files(id,name,mimeType,size,modifiedTime,webViewLink),nextPageToken', supportsAllDrives: true, includeItemsFromAllDrives: true } });
    return page(result.files, result.nextPageToken);
  }),
  get_file: action('Get file metadata', 'Read file metadata without downloading contents', 'read', { fileId: id() }, ['fileId'], (ctx, input) => ctx.request({ path: `/drive/v3/files/${segment(input.fileId)}`, query: { fields: 'id,name,mimeType,size,modifiedTime,webViewLink,capabilities', supportsAllDrives: true } })),
  download_file: action('Download file', 'Read binary file contents as base64, up to 10 MiB. Use export_file for Google documents.', 'read', { fileId: id() }, ['fileId'], (ctx, input) => ctx.request({ path: `/drive/v3/files/${segment(input.fileId)}`, query: { alt: 'media', supportsAllDrives: true }, response: 'binary' })),
  export_file: action('Export Google document', 'Export a Workspace document to an explicit MIME type, up to 10 MiB', 'read', { fileId: id(), mimeType: string() }, ['fileId', 'mimeType'], (ctx, input) => ctx.request({ path: `/drive/v3/files/${segment(input.fileId)}/export`, query: { mimeType: input.mimeType }, response: 'binary' })),
  update_file: action('Update file metadata', 'Rename a file or move it to/from the trash without permanently deleting it', 'write', { fileId: id(), name: string(), trashed: boolean }, ['fileId'], (ctx, input) => {
    if (input.name === undefined && input.trashed === undefined) throw new Error('Specify name or trashed');
    return ctx.request({ path: `/drive/v3/files/${segment(input.fileId)}`, method: 'PATCH', query: { supportsAllDrives: true, fields: 'id,name,trashed' }, json: { ...(input.name !== undefined ? { name: input.name } : {}), ...(input.trashed !== undefined ? { trashed: input.trashed } : {}) } });
  }),
  upload_file: action('Upload file', 'Create a file with metadata and base64 contents, up to 1 MiB decoded. Reconcile uncertain writes.', 'write', { name: string(), mimeType: string('MIME type', { pattern: '^[A-Za-z0-9.+-]+/[A-Za-z0-9.+-]+$' }), parentId: id(), contentBase64: string('Base64-encoded file bytes', { minLength: 0, maxLength: 1398104, pattern: '^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$' }) }, ['name', 'mimeType', 'contentBase64'], (ctx, input) => {
    const boundary = `rootcx_${crypto.randomUUID()}`;
    const metadata = JSON.stringify({ name: input.name, ...(input.parentId ? { parents: [input.parentId] } : {}) });
    const body = new Blob([`--${boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n${metadata}\r\n--${boundary}\r\nContent-Type: ${input.mimeType}\r\n\r\n`, Buffer.from(input.contentBase64, 'base64'), `\r\n--${boundary}--\r\n`]);
    return ctx.request({ path: '/upload/drive/v3/files', method: 'POST', query: { uploadType: 'multipart', supportsAllDrives: true, fields: 'id,name,mimeType,size' }, headers: { 'Content-Type': `multipart/related; boundary=${boundary}` }, body });
  }),
}, { services: googleServices, oauth, discovery: { action: 'list_files', itemsPath: 'items', idPath: 'id', labelPath: 'name' } });
