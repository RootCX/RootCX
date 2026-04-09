import { describe, it, expect, vi, beforeEach } from "vitest";
import { handleWebhook, type WebhookDeps } from "./webhook-handler";

/**
 * Webhook handler tests — verifies that large data (XML, attachments)
 * goes through uploadFile (Storage), not inline in collectionOp (IPC).
 *
 * What this catches: someone puts xml/attachment content back inline
 * in collectionOp → IPC overflow → worker crash → data loss (the 07 Apr incident).
 */

const uploadCalls: Array<{ filename: string; size: number }> = [];
const insertCalls: Array<{ entity: string; data: Record<string, unknown> }> = [];
const confirmCalls: string[] = [];

const deps: WebhookDeps = {
  uploadFile: vi.fn(async (content, filename) => {
    uploadCalls.push({ filename, size: content?.length ?? content?.byteLength ?? 0 });
    return `file-${uploadCalls.length}`;
  }),
  collectionOp: vi.fn(async (op, entity, data) => {
    insertCalls.push({ entity, data });
    return { id: "rec-1" };
  }),
  dokapiRequest: vi.fn(async (_cfg, _method, endpoint) => {
    if (endpoint.includes("/confirm")) confirmCalls.push(endpoint);
    return {};
  }),
};

// Minimal UBL with an embedded PDF attachment
const UBL = `<?xml version="1.0"?>
<Invoice xmlns="urn:oasis:names:specification:ubl:schema:xsd:Invoice-2"
         xmlns:cac="urn:oasis:names:specification:ubl:schema:xsd:CommonAggregateComponents-2"
         xmlns:cbc="urn:oasis:names:specification:ubl:schema:xsd:CommonBasicComponents-2">
  <cbc:ID>TEST-001</cbc:ID>
  <cbc:IssueDate>2026-04-09</cbc:IssueDate>
  <cbc:InvoiceTypeCode>380</cbc:InvoiceTypeCode>
  <cbc:DocumentCurrencyCode>EUR</cbc:DocumentCurrencyCode>
  <cac:AdditionalDocumentReference>
    <cac:Attachment>
      <cbc:EmbeddedDocumentBinaryObject mimeCode="application/pdf" filename="invoice.pdf">${btoa("fake-pdf-content")}</cbc:EmbeddedDocumentBinaryObject>
    </cac:Attachment>
  </cac:AdditionalDocumentReference>
  <cac:AccountingSupplierParty><cac:Party><cac:PartyName><cbc:Name>Supplier</cbc:Name></cac:PartyName>
    <cac:PartyIdentification><cbc:ID schemeID="0208">0208:0000000001</cbc:ID></cac:PartyIdentification>
  </cac:Party></cac:AccountingSupplierParty>
  <cac:AccountingCustomerParty><cac:Party><cac:PartyName><cbc:Name>Buyer</cbc:Name></cac:PartyName>
    <cac:PartyIdentification><cbc:ID schemeID="0208">0208:0000000002</cbc:ID></cac:PartyIdentification>
  </cac:Party></cac:AccountingCustomerParty>
  <cac:LegalMonetaryTotal><cbc:PayableAmount currencyID="EUR">100.00</cbc:PayableAmount></cac:LegalMonetaryTotal>
</Invoice>`;

vi.stubGlobal("fetch", vi.fn(async () => ({ ok: true, text: async () => UBL })));

const PARAMS = {
  body: {
    event: "incoming-peppol-documents.received",
    body: {
      ulid: "01TESTWEBHOOK001",
      status: "RECEIVED",
      presignedUrl: "http://localhost/fake.xml",
      sender: { scheme: "iso6523-actorid-upis", value: "0208:0000000001" },
      receiver: { scheme: "iso6523-actorid-upis", value: "0208:0000000002" },
      validationStatus: "VALID",
    },
  },
  config: {},
};

beforeEach(() => {
  uploadCalls.length = 0;
  insertCalls.length = 0;
  confirmCalls.length = 0;
  vi.clearAllMocks();
});

describe("incoming webhook: XML and attachments go through Storage", () => {
  it("xml field in collectionOp is a file ID, not XML content", async () => {
    const result = await handleWebhook(PARAMS, deps);
    expect(result.status).toBe("received");

    const insert = insertCalls.find((c) => c.entity === "incoming_documents");
    expect(insert).toBeDefined();

    const xmlField = insert!.data.xml as string;
    expect(xmlField).toMatch(/^file-/);
    expect(xmlField).not.toContain("<?xml");
  });

  it("attachments contain fileId references, not base64 content", async () => {
    await handleWebhook(PARAMS, deps);

    const insert = insertCalls.find((c) => c.entity === "incoming_documents");
    const attachments = insert!.data.attachments as any[];
    expect(attachments.length).toBeGreaterThan(0);
    expect(attachments[0].fileId).toMatch(/^file-/);
    expect(attachments[0]).not.toHaveProperty("base64Content");
  });

  it("uploadFile called for XML and each attachment", async () => {
    await handleWebhook(PARAMS, deps);

    expect(uploadCalls.some((c) => c.filename.endsWith(".xml"))).toBe(true);
    expect(uploadCalls.some((c) => c.filename === "invoice.pdf")).toBe(true);
  });

  it("confirms with Dokapi only after successful persist", async () => {
    await handleWebhook(PARAMS, deps);

    expect(confirmCalls).toHaveLength(1);
    expect(confirmCalls[0]).toContain("01TESTWEBHOOK001");
  });

  it("does NOT confirm if persist fails", async () => {
    const failDeps: WebhookDeps = {
      ...deps,
      collectionOp: vi.fn(async () => { throw new Error("DB down"); }),
    };

    const result = await handleWebhook(PARAMS, failDeps);
    expect(result.status).toBe("persist_failed");
    expect(confirmCalls).toHaveLength(0);
  });
});
