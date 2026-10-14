import { provider, googleOAuth, googleServices } from './shared';
import { action, array, id, pagination, page, segment, string } from '../schema';

const values = array(array({ type: ['string', 'number', 'boolean', 'null'] }, 1000), 1000);
const target = { spreadsheetId: id(), range: string('A1 range including sheet name, e.g. Sheet1!A1:C10') };
export default provider('google_sheets', 'Google Sheets', 'https://sheets.googleapis.com', {
  list_spreadsheets: action('List spreadsheets', 'Discover accessible spreadsheet files', 'read', pagination, [], async (ctx, input) => {
    const result = await ctx.request({ service: 'drive', path: '/drive/v3/files', query: { q: "mimeType = 'application/vnd.google-apps.spreadsheet' and trashed = false", pageSize: input.limit ?? 50, pageToken: input.cursor, fields: 'files(id,name),nextPageToken', supportsAllDrives: true, includeItemsFromAllDrives: true } });
    return page(result.files, result.nextPageToken);
  }),
  get_schema: action('Get spreadsheet structure', 'Read worksheet IDs, titles and dimensions', 'read', { spreadsheetId: id() }, ['spreadsheetId'], (ctx, input) => ctx.request({ path: `/v4/spreadsheets/${segment(input.spreadsheetId)}`, query: { fields: 'spreadsheetId,properties,sheets.properties' } })),
  read_range: action('Read range', 'Read a bounded A1 range; ranges are coordinates, not stable record IDs', 'read', target, ['spreadsheetId', 'range'], (ctx, input) => ctx.request({ path: `/v4/spreadsheets/${segment(input.spreadsheetId)}/values/${segment(input.range)}` })),
  append_rows: action('Append rows', 'Append rows to a table. Reconcile uncertain writes; never retry automatically.', 'write', { ...target, values }, ['spreadsheetId', 'range', 'values'], (ctx, input) => ctx.request({ path: `/v4/spreadsheets/${segment(input.spreadsheetId)}/values/${segment(input.range)}:append`, method: 'POST', query: { valueInputOption: 'RAW', insertDataOption: 'INSERT_ROWS' }, json: { values: input.values, majorDimension: 'ROWS' } })),
  update_range: action('Update range', 'Write RAW values to explicit coordinates; empty string clears a cell and null skips it', 'write', { ...target, values }, ['spreadsheetId', 'range', 'values'], (ctx, input) => ctx.request({ path: `/v4/spreadsheets/${segment(input.spreadsheetId)}/values/${segment(input.range)}`, method: 'PUT', query: { valueInputOption: 'RAW' }, json: { values: input.values, majorDimension: 'ROWS' } })),
}, { services: { ...googleServices, drive: 'https://www.googleapis.com' }, oauth: googleOAuth(['https://www.googleapis.com/auth/spreadsheets', 'https://www.googleapis.com/auth/drive.metadata.readonly']), discovery: { action: 'list_spreadsheets', itemsPath: 'items', idPath: 'id', labelPath: 'name' } });
