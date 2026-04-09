import { createHmac, timingSafeEqual } from "crypto";
import { parseUbl, type ParsedUBLDocument } from "./parseUbl";
import { handleWebhook as handleWebhookImpl, type WebhookDeps } from "./webhook-handler";

// ─── Types ───────────────────────────────────────────────────────────────────

interface Config {
  clientId: string;
  clientSecret: string;
  baseUrl?: string;
  identityUrl?: string;
  webhookSecret?: string;
  proxyToken?: string;
}

interface InvoiceParty {
  peppolId: string;
  name: string;
  vatNumber: string;
  street: string;
  city: string;
  postalCode: string;
  countryCode: string;
  contactName?: string;
  contactEmail?: string;
  contactPhone?: string;
}

interface InvoiceLine {
  id: string;
  description: string;
  quantity: number;
  unitCode?: string;
  unitPrice: number;
  taxPercent: number;
  taxCategory?: string;
  lineAmount: number;
}

interface PaymentInfo {
  iban: string;
  bic?: string;
  bankName?: string;
  accountName?: string;
}

interface EmbeddedAttachment {
  base64Content: string;
  mimeCode: string;
  filename: string;
}

interface DocumentReference {
  id: string;
  typeCode?: string;
  description?: string;
  attachment?: EmbeddedAttachment;
}

interface InvoiceParams {
  invoiceNumber: string;
  issueDate: string;
  dueDate: string;
  currency?: string;
  buyerReference?: string;
  orderReference?: string;
  contractReference?: string;
  projectReference?: string;
  originatorReference?: string;
  documentReferences?: DocumentReference[];
  supplier: InvoiceParty;
  customer: InvoiceParty;
  lines: InvoiceLine[];
  taxTotal: number;
  taxableAmount: number;
  payableAmount: number;
  note?: string;
  paymentInfo?: PaymentInfo;
}

interface PeppolCountryConfig {
  icd: string;
  identifierDigits: number;
}


// ─── Country Configs ─────────────────────────────────────────────────────────

const BIS3_INVOICE = "urn:oasis:names:specification:ubl:schema:xsd:Invoice-2::Invoice##urn:cen.eu:en16931:2017#compliant#urn:fdc:peppol.eu:2017:poacc:billing:3.0::2.1";
const BIS3_CREDIT = "urn:oasis:names:specification:ubl:schema:xsd:CreditNote-2::CreditNote##urn:cen.eu:en16931:2017#compliant#urn:fdc:peppol.eu:2017:poacc:billing:3.0::2.1";
const BIS3_PROCESS = "urn:fdc:peppol.eu:2017:poacc:billing:01:1.0";
const BIS3_IMR_DOCTYPE = "urn:oasis:names:specification:ubl:schema:xsd:ApplicationResponse-2::ApplicationResponse##urn:fdc:peppol.eu:poacc:trns:invoice_response:3::2.1";
const BIS3_IMR_PROCESS = "urn:fdc:peppol.eu:poacc:bis:invoice_response:3";

const STANDARD_DOC_TYPES = [
  { name: "BIS Billing 3.0 Invoice", documentTypeIdentifier: BIS3_INVOICE, processIdentifier: BIS3_PROCESS },
  { name: "BIS Billing 3.0 Credit Note", documentTypeIdentifier: BIS3_CREDIT, processIdentifier: BIS3_PROCESS },
];

const COUNTRY_CONFIGS: Record<string, PeppolCountryConfig> = {
  BE: { icd: "0208", identifierDigits: 10 },
  NL: { icd: "0106", identifierDigits: 8 },
  FR: { icd: "0009", identifierDigits: 14 },
  DE: { icd: "0204", identifierDigits: 9 },
  IT: { icd: "0211", identifierDigits: 11 },
  ES: { icd: "9920", identifierDigits: 9 },
  AT: { icd: "9915", identifierDigits: 9 },
  LU: { icd: "9938", identifierDigits: 13 },
  PT: { icd: "9946", identifierDigits: 9 },
  IE: { icd: "9952", identifierDigits: 8 },
  FI: { icd: "0037", identifierDigits: 8 },
  SE: { icd: "0007", identifierDigits: 10 },
  NO: { icd: "0192", identifierDigits: 9 },
  DK: { icd: "0184", identifierDigits: 8 },
  PL: { icd: "9945", identifierDigits: 10 },
  CZ: { icd: "9956", identifierDigits: 8 },
  RO: { icd: "9947", identifierDigits: 10 },
  BG: { icd: "9926", identifierDigits: 9 },
  HR: { icd: "9958", identifierDigits: 11 },
  SI: { icd: "9948", identifierDigits: 8 },
  SK: { icd: "9949", identifierDigits: 10 },
  EE: { icd: "9931", identifierDigits: 8 },
  LV: { icd: "9932", identifierDigits: 11 },
  LT: { icd: "9933", identifierDigits: 9 },
  HU: { icd: "9910", identifierDigits: 8 },
  CY: { icd: "9955", identifierDigits: 9 },
  MT: { icd: "9934", identifierDigits: 8 },
  GR: { icd: "9923", identifierDigits: 9 },
  SG: { icd: "0195", identifierDigits: 10 },
  AU: { icd: "0151", identifierDigits: 11 },
  NZ: { icd: "0088", identifierDigits: 13 },
  US: { icd: "0199", identifierDigits: 10 },
  JP: { icd: "0221", identifierDigits: 13 },
};

function getCountryConfig(countryCode: string): PeppolCountryConfig {
  return COUNTRY_CONFIGS[countryCode.toUpperCase()] ?? COUNTRY_CONFIGS.BE;
}

// ─── Dokapi Client ───────────────────────────────────────────────────────────

const USER_AGENT = "RootCX/2.0";
let cachedToken: { token: string; expiresAt: number } | null = null;

const DEFAULT_BASE = "https://peppol-api.dokapi.io/v1";
const DEFAULT_IDENTITY = "https://portal.dokapi.io/api/oauth2/token";

async function getAccessToken(config: Config): Promise<string | null> {
  if (!config.clientId || !config.clientSecret) return config.proxyToken || null;
  if (cachedToken && Date.now() < cachedToken.expiresAt - 60_000) return cachedToken.token;

  const res = await fetch(config.identityUrl || DEFAULT_IDENTITY, {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded", "User-Agent": USER_AGENT },
    body: new URLSearchParams({
      grant_type: "client_credentials",
      client_id: config.clientId,
      client_secret: config.clientSecret,
      scope: "peppol_api",
    }),
  });
  if (!res.ok) throw new Error(`Dokapi auth failed: ${await res.text()}`);
  const data = await res.json();
  cachedToken = { token: data.access_token, expiresAt: Date.now() + data.expires_in * 1000 };
  return data.access_token;
}

async function dokapiRequest<T>(config: Config, method: string, endpoint: string, body?: unknown): Promise<T> {
  const token = await getAccessToken(config);
  const headers: Record<string, string> = { "Content-Type": "application/json", "User-Agent": USER_AGENT };
  if (token) headers.Authorization = `Bearer ${token}`;
  const res = await fetch(`${config.baseUrl || DEFAULT_BASE}${endpoint}`, {
    method,
    headers,
    body: body ? JSON.stringify(body) : undefined,
  });
  if (!res.ok) {
    const ct = res.headers.get("content-type");
    const errMsg = ct?.includes("application/json")
      ? JSON.stringify(await res.json().catch(() => res.statusText))
      : await res.text();
    throw new Error(`Dokapi API ${res.status}: ${errMsg}`);
  }
  if (res.status === 204 || !res.headers.get("content-type")?.includes("application/json")) return undefined as T;
  const text = await res.text();
  return text ? JSON.parse(text) : (undefined as T);
}

function formatToPeppolId(identifier: string, countryConfig: PeppolCountryConfig): string {
  const clean = identifier.replace(/[^0-9]/g, "");
  return `${countryConfig.icd}:${clean.padStart(countryConfig.identifierDigits, "0")}`;
}

// ─── UBL Generator ───────────────────────────────────────────────────────────

function escapeXml(str: string): string {
  return str.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;").replace(/'/g, "&apos;");
}

function extractIdentifier(peppolId: string): string {
  const value = peppolId.split(":")[1] || peppolId;
  return value.replace(/^[A-Z]{2}/i, "");
}

function formatVat(vatNumber: string, countryCode: string): string {
  const clean = vatNumber.replace(/[^0-9A-Z]/gi, "").toUpperCase();
  return clean.match(/^[A-Z]{2}/) ? clean : `${countryCode}${clean}`;
}

function generateInvoiceXml(params: InvoiceParams): string {
  const {
    invoiceNumber, issueDate, dueDate, currency = "EUR",
    buyerReference = invoiceNumber, orderReference, supplier, customer,
    lines, taxTotal, taxableAmount, payableAmount, note,
  } = params;

  const supplierId = extractIdentifier(supplier.peppolId);
  const customerId = extractIdentifier(customer.peppolId);
  const supplierVat = formatVat(supplier.vatNumber, supplier.countryCode);
  const customerVat = formatVat(customer.vatNumber, customer.countryCode);
  const resolveTaxCategory = (cat: string | undefined, pct: number) => cat || (pct === 0 ? "E" : "S");

  // Group lines by tax category + rate for TaxSubtotal breakdown
  const taxGroups = lines.reduce((m, l) => {
    const cat = resolveTaxCategory(l.taxCategory, l.taxPercent), pct = l.taxPercent ?? 21, key = `${cat}:${pct}`;
    const g = m.get(key) || { category: cat, percent: pct, taxableAmount: 0 };
    g.taxableAmount += l.lineAmount;
    return m.set(key, g);
  }, new Map<string, { category: string; percent: number; taxableAmount: number }>());

  const invoiceLines = lines.map((l) => `
    <cac:InvoiceLine>
        <cbc:ID>${escapeXml(l.id)}</cbc:ID>
        <cbc:InvoicedQuantity unitCode="${l.unitCode || "C62"}">${l.quantity}</cbc:InvoicedQuantity>
        <cbc:LineExtensionAmount currencyID="${currency}">${l.lineAmount.toFixed(2)}</cbc:LineExtensionAmount>
        <cac:Item>
            <cbc:Name>${escapeXml(l.description)}</cbc:Name>
            <cac:ClassifiedTaxCategory>
                <cbc:ID>${resolveTaxCategory(l.taxCategory, l.taxPercent)}</cbc:ID>
                <cbc:Percent>${l.taxPercent}</cbc:Percent>
                <cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme>
            </cac:ClassifiedTaxCategory>
        </cac:Item>
        <cac:Price><cbc:PriceAmount currencyID="${currency}">${l.unitPrice.toFixed(2)}</cbc:PriceAmount></cac:Price>
    </cac:InvoiceLine>`).join("");

  const noteEl = note ? `\n    <cbc:Note>${escapeXml(note)}</cbc:Note>` : "";
  const orderRefEl = orderReference ? `\n    <cac:OrderReference><cbc:ID>${escapeXml(orderReference)}</cbc:ID></cac:OrderReference>` : "";
  const contractRefEl = params.contractReference ? `\n    <cac:ContractDocumentReference><cbc:ID>${escapeXml(params.contractReference)}</cbc:ID></cac:ContractDocumentReference>` : "";
  const projectRefEl = params.projectReference ? `\n    <cac:ProjectReference><cbc:ID>${escapeXml(params.projectReference)}</cbc:ID></cac:ProjectReference>` : "";
  const originatorRefEl = params.originatorReference ? `\n    <cac:OriginatorDocumentReference><cbc:ID>${escapeXml(params.originatorReference)}</cbc:ID></cac:OriginatorDocumentReference>` : "";

  const docRefEls = params.documentReferences?.map((ref) => `
    <cac:AdditionalDocumentReference>
        <cbc:ID>${escapeXml(ref.id)}</cbc:ID>${ref.typeCode ? `\n        <cbc:DocumentTypeCode>${escapeXml(ref.typeCode)}</cbc:DocumentTypeCode>` : ""}${ref.description ? `\n        <cbc:DocumentDescription>${escapeXml(ref.description)}</cbc:DocumentDescription>` : ""}${ref.attachment ? `
        <cac:Attachment>
            <cbc:EmbeddedDocumentBinaryObject mimeCode="${ref.attachment.mimeCode}" filename="${escapeXml(ref.attachment.filename)}">${ref.attachment.base64Content}</cbc:EmbeddedDocumentBinaryObject>
        </cac:Attachment>` : ""}
    </cac:AdditionalDocumentReference>`).join("") || "";

  const customerContactEl = (customer.contactName || customer.contactEmail || customer.contactPhone)
    ? `\n            <cac:Contact>${customer.contactName ? `\n                <cbc:Name>${escapeXml(customer.contactName)}</cbc:Name>` : ""}${customer.contactPhone ? `\n                <cbc:Telephone>${escapeXml(customer.contactPhone)}</cbc:Telephone>` : ""}${customer.contactEmail ? `\n                <cbc:ElectronicMail>${escapeXml(customer.contactEmail)}</cbc:ElectronicMail>` : ""}\n            </cac:Contact>` : "";

  const paymentMeansEl = params.paymentInfo?.iban ? `
    <cac:PaymentMeans>
        <cbc:PaymentMeansCode>30</cbc:PaymentMeansCode>
        <cbc:PaymentID>${escapeXml(invoiceNumber)}</cbc:PaymentID>
        <cac:PayeeFinancialAccount>
            <cbc:ID>${escapeXml(params.paymentInfo.iban)}</cbc:ID>${params.paymentInfo.accountName ? `\n            <cbc:Name>${escapeXml(params.paymentInfo.accountName)}</cbc:Name>` : ""}${params.paymentInfo.bic ? `
            <cac:FinancialInstitutionBranch>
                <cbc:ID>${escapeXml(params.paymentInfo.bic)}</cbc:ID>
            </cac:FinancialInstitutionBranch>` : ""}
        </cac:PayeeFinancialAccount>
    </cac:PaymentMeans>` : "";

  const schemeId = supplier.peppolId.split(":")[0] || "0208";

  return `<?xml version="1.0" encoding="UTF-8"?>
<Invoice xmlns="urn:oasis:names:specification:ubl:schema:xsd:Invoice-2"
         xmlns:cac="urn:oasis:names:specification:ubl:schema:xsd:CommonAggregateComponents-2"
         xmlns:cbc="urn:oasis:names:specification:ubl:schema:xsd:CommonBasicComponents-2">
    <cbc:CustomizationID>urn:cen.eu:en16931:2017#compliant#urn:fdc:peppol.eu:2017:poacc:billing:3.0</cbc:CustomizationID>
    <cbc:ProfileID>urn:fdc:peppol.eu:2017:poacc:billing:01:1.0</cbc:ProfileID>
    <cbc:ID>${escapeXml(invoiceNumber)}</cbc:ID>
    <cbc:IssueDate>${issueDate}</cbc:IssueDate>
    <cbc:DueDate>${dueDate}</cbc:DueDate>
    <cbc:InvoiceTypeCode>380</cbc:InvoiceTypeCode>${noteEl}
    <cbc:DocumentCurrencyCode>${currency}</cbc:DocumentCurrencyCode>
    <cbc:BuyerReference>${escapeXml(buyerReference)}</cbc:BuyerReference>${orderRefEl}${originatorRefEl}${contractRefEl}${docRefEls}${projectRefEl}
    <cac:AccountingSupplierParty>
        <cac:Party>
            <cbc:EndpointID schemeID="${schemeId}">${supplierId}</cbc:EndpointID>
            <cac:PartyName><cbc:Name>${escapeXml(supplier.name)}</cbc:Name></cac:PartyName>
            <cac:PostalAddress>
                <cbc:StreetName>${escapeXml(supplier.street)}</cbc:StreetName>
                <cbc:CityName>${escapeXml(supplier.city)}</cbc:CityName>
                <cbc:PostalZone>${escapeXml(supplier.postalCode)}</cbc:PostalZone>
                <cac:Country><cbc:IdentificationCode>${supplier.countryCode}</cbc:IdentificationCode></cac:Country>
            </cac:PostalAddress>
            <cac:PartyTaxScheme>
                <cbc:CompanyID>${supplierVat}</cbc:CompanyID>
                <cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme>
            </cac:PartyTaxScheme>
            <cac:PartyLegalEntity>
                <cbc:RegistrationName>${escapeXml(supplier.name)}</cbc:RegistrationName>
                <cbc:CompanyID schemeID="${schemeId}">${supplierId}</cbc:CompanyID>
            </cac:PartyLegalEntity>
        </cac:Party>
    </cac:AccountingSupplierParty>
    <cac:AccountingCustomerParty>
        <cac:Party>
            <cbc:EndpointID schemeID="${customer.peppolId.split(":")[0] || schemeId}">${customerId}</cbc:EndpointID>
            <cac:PartyName><cbc:Name>${escapeXml(customer.name)}</cbc:Name></cac:PartyName>
            <cac:PostalAddress>
                <cbc:StreetName>${escapeXml(customer.street)}</cbc:StreetName>
                <cbc:CityName>${escapeXml(customer.city)}</cbc:CityName>
                <cbc:PostalZone>${escapeXml(customer.postalCode)}</cbc:PostalZone>
                <cac:Country><cbc:IdentificationCode>${customer.countryCode}</cbc:IdentificationCode></cac:Country>
            </cac:PostalAddress>
            <cac:PartyTaxScheme>
                <cbc:CompanyID>${customerVat}</cbc:CompanyID>
                <cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme>
            </cac:PartyTaxScheme>
            <cac:PartyLegalEntity>
                <cbc:RegistrationName>${escapeXml(customer.name)}</cbc:RegistrationName>
            </cac:PartyLegalEntity>${customerContactEl}
        </cac:Party>
    </cac:AccountingCustomerParty>${paymentMeansEl}
    <cac:TaxTotal>
        <cbc:TaxAmount currencyID="${currency}">${taxTotal.toFixed(2)}</cbc:TaxAmount>${Array.from(taxGroups.values()).map(g => {
      const groupTax = Math.round(g.taxableAmount * g.percent) / 100;
      return `
        <cac:TaxSubtotal>
            <cbc:TaxableAmount currencyID="${currency}">${g.taxableAmount.toFixed(2)}</cbc:TaxableAmount>
            <cbc:TaxAmount currencyID="${currency}">${groupTax.toFixed(2)}</cbc:TaxAmount>
            <cac:TaxCategory>
                <cbc:ID>${g.category}</cbc:ID>
                <cbc:Percent>${g.percent}</cbc:Percent>${g.category === "E" ? `
                <cbc:TaxExemptionReasonCode>vatex-eu-132</cbc:TaxExemptionReasonCode>
                <cbc:TaxExemptionReason>Exempt</cbc:TaxExemptionReason>` : ""}
                <cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme>
            </cac:TaxCategory>
        </cac:TaxSubtotal>`;
    }).join("")}
    </cac:TaxTotal>
    <cac:LegalMonetaryTotal>
        <cbc:LineExtensionAmount currencyID="${currency}">${taxableAmount.toFixed(2)}</cbc:LineExtensionAmount>
        <cbc:TaxExclusiveAmount currencyID="${currency}">${taxableAmount.toFixed(2)}</cbc:TaxExclusiveAmount>
        <cbc:TaxInclusiveAmount currencyID="${currency}">${payableAmount.toFixed(2)}</cbc:TaxInclusiveAmount>
        <cbc:PayableAmount currencyID="${currency}">${payableAmount.toFixed(2)}</cbc:PayableAmount>
    </cac:LegalMonetaryTotal>${invoiceLines}
</Invoice>`;
}

function generateTestInvoiceXml(peppolId: string, vatNumber: string): { xml: string; invoiceNumber: string } {
  const today = new Date().toISOString().split("T")[0];
  const invoiceNumber = `TEST-${Date.now()}`;
  const xml = generateInvoiceXml({
    invoiceNumber, issueDate: today, dueDate: today, currency: "EUR",
    supplier: { peppolId, name: "Test Sender", vatNumber, street: "Test Street 1", city: "Brussels", postalCode: "1000", countryCode: "BE" },
    customer: { peppolId, name: "Test Receiver", vatNumber, street: "Test Street 2", city: "Brussels", postalCode: "1000", countryCode: "BE" },
    lines: [{ id: "1", description: "Test Product", quantity: 1, unitPrice: 100, taxPercent: 0, taxCategory: "E", lineAmount: 100 }],
    taxTotal: 0, taxableAmount: 100, payableAmount: 100, note: "Test invoice - do not process",
  });
  return { xml, invoiceNumber };
}

// ─── Action Handlers ─────────────────────────────────────────────────────────

async function registerParticipant(config: Config, input: any) {
  const { vatNumber, companyName, countryCode = "BE" } = input;
  if (!vatNumber) throw new Error("vatNumber is required");
  if (!companyName) throw new Error("companyName is required");

  const cc = countryCode.toUpperCase();
  const peppolId = formatToPeppolId(vatNumber, getCountryConfig(cc));
  const pid = { scheme: "iso6523-actorid-upis", value: peppolId };

  let reg: any;
  try {
    const res = await dokapiRequest<any>(config, "POST", "/participant-registrations", {
      participantIdentifier: pid, countryCode: cc,
      completeBusinessCard: { businessEntity: [{
        name: [{ value: companyName }], countryCode: cc,
        identifier: [{ scheme: `${cc}:VAT`, value: vatNumber.replace(/[^0-9A-Z]/gi, "").toUpperCase() }],
      }] },
    });
    reg = res?.participantRegistration;
  } catch (err: any) {
    if (!err.message?.includes("409")) throw err;
    // 409 — already registered, try scoped lookup (may fail on proxy ownership check)
    try { reg = await dokapiRequest<any>(config, "GET", `/participant-registrations/find?scheme=${pid.scheme}&value=${encodeURIComponent(pid.value)}`); }
    catch { return { peppolId, dokapiUlid: null, status: "active" }; }
  }

  try { await dokapiRequest(config, "POST", "/participant-registrations/business-cards/push", pid); } catch {}

  return { peppolId, dokapiUlid: reg?.ulid, status: reg?.status || "active" };
}

async function deregisterParticipant(config: Config, input: any) {
  const { peppolId } = input;
  if (!peppolId) throw new Error("peppolId is required");
  try {
    await dokapiRequest(config, "DELETE", "/participant-registrations", {
      scheme: "iso6523-actorid-upis", value: peppolId,
    });
  } catch (err: any) {
    if (!err.message?.includes("404")) throw err;
  }
  return { success: true };
}

async function registerDocumentTypes(config: Config, input: any) {
  const { peppolId } = input;
  if (!peppolId) throw new Error("peppolId is required");

  const registered: string[] = [];

  for (const docType of STANDARD_DOC_TYPES) {
    try {
      await dokapiRequest(config, "POST", "/participant-registrations/documents", {
        participantIdentifier: { scheme: "iso6523-actorid-upis", value: peppolId },
        documentTypeIdentifier: { scheme: "busdox-docid-qns", value: docType.documentTypeIdentifier },
        processIdentifier: { scheme: "cenbii-procid-ubl", value: docType.processIdentifier },
      });
      registered.push(docType.name);
    } catch (err: any) {
      if (err.message?.includes("409")) registered.push(`${docType.name} (already registered)`);
      else throw err;
    }
  }
  return { documentTypes: registered };
}

async function refreshParticipantStatus(config: Config, input: any) {
  const { peppolId } = input;
  if (!peppolId) throw new Error("peppolId is required");
  const result = await dokapiRequest<any>(config, "GET",
    `/participant-registrations/find?scheme=iso6523-actorid-upis&value=${encodeURIComponent(peppolId)}`);
  return {
    status: (result?.status || "").toLowerCase() || "unknown",
    peppolId: result?.participantIdentifier?.value,
  };
}

async function sendInvoice(config: Config, input: any) {
  const { senderPeppolId, receiverPeppolId, countryCode = "BE" } = input;
  if (!senderPeppolId) throw new Error("senderPeppolId is required");
  if (!receiverPeppolId) throw new Error("receiverPeppolId is required");

  let xml = input.xml;
  if (!xml && input.invoiceParams) xml = generateInvoiceXml(input.invoiceParams);
  if (!xml) throw new Error("Either xml or invoiceParams must be provided");

  const [senderScheme, senderRaw] = senderPeppolId.split(":");
  const [receiverScheme, receiverRaw] = receiverPeppolId.split(":");
  const senderValue = extractIdentifier(senderRaw || "");
  const receiverValue = extractIdentifier(receiverRaw || "");

  const sendResponse = await dokapiRequest<any>(config, "POST", "/outgoing-peppol-documents", {
    c1CountryCode: countryCode.toUpperCase(),
    sender: { scheme: "iso6523-actorid-upis", value: `${senderScheme}:${senderValue}` },
    receiver: { scheme: "iso6523-actorid-upis", value: `${receiverScheme}:${receiverValue}` },
    documentTypeIdentifier: { scheme: "busdox-docid-qns", value: BIS3_INVOICE },
    processIdentifier: { scheme: "cenbii-procid-ubl", value: BIS3_PROCESS },
  });

  // Upload UBL XML to pre-signed URL
  const uploadRes = await fetch(sendResponse.preSignedUploadUrl, {
    method: "PUT",
    headers: { "Content-Type": "application/xml" },
    body: xml,
  });
  if (!uploadRes.ok) throw new Error(`Failed to upload document: ${uploadRes.status}`);

  return {
    dokapiUlid: sendResponse.document.ulid,
    status: sendResponse.document.status || "sending",
  };
}

async function sendTestInvoice(config: Config, input: any) {
  const { peppolId, vatNumber } = input;
  if (!peppolId || !vatNumber) throw new Error("peppolId and vatNumber are required");
  const { xml, invoiceNumber } = generateTestInvoiceXml(peppolId, vatNumber);
  const result = await sendInvoice(config, { senderPeppolId: peppolId, receiverPeppolId: peppolId, xml });
  return { ...result, invoiceNumber };
}

async function validateDocument(config: Config, input: any) {
  const { xml } = input;
  if (!xml) throw new Error("xml is required");
  try {
    const result = await dokapiRequest<any>(config, "POST", "/validating-peppol-documents", { content: xml });
    const isValid = result?.status === "VALID" || result?.valid === true;
    if (!isValid) {
      const errors: string[] = [];
      if (Array.isArray(result?.errors)) {
        for (const err of result.errors) {
          const msg = typeof err === "string" ? err : (err.errorMessage || err.message);
          if (msg) errors.push(msg);
        }
      }
      return { valid: false, errors: errors.length > 0 ? errors : ["Invalid UBL document"] };
    }
    return { valid: true, errors: [] };
  } catch {
    return { valid: false, errors: ["Validation service unavailable"] };
  }
}

function generateUbl(_config: Config, input: any) {
  const { invoiceParams } = input;
  if (!invoiceParams) throw new Error("invoiceParams is required");
  return { xml: generateInvoiceXml(invoiceParams) };
}

function parseUblAction(_config: Config, input: any) {
  const { xml } = input;
  if (!xml) throw new Error("xml is required");
  return parseUbl(xml);
}

async function listWebhooks(config: Config) {
  const webhooks = await dokapiRequest<any>(config, "GET", "/webhooks");
  return { webhooks: webhooks || [] };
}

async function registerWebhook(config: Config, input: any) {
  const { url, events } = input;
  if (!url || !events?.length) throw new Error("url and events are required");
  return dokapiRequest(config, "POST", "/webhooks", { url, events });
}

async function deleteWebhook(config: Config, input: any) {
  const { ulid } = input;
  if (!ulid) throw new Error("ulid is required");
  await dokapiRequest(config, "DELETE", `/webhooks/${ulid}`);
  return { success: true };
}

async function downloadDocument(_config: Config, input: any) {
  const { presignedUrl } = input;
  if (!presignedUrl) throw new Error("presignedUrl is required");
  const res = await fetch(presignedUrl, { headers: { "User-Agent": USER_AGENT } });
  if (!res.ok) throw new Error(`Failed to download document: ${res.status}`);
  const xml = await res.text();
  return { xml, parsed: parseUbl(xml) };
}

async function confirmDocumentDownload(config: Config, input: any) {
  const { documentUlid } = input;
  if (!documentUlid) throw new Error("documentUlid is required");
  await dokapiRequest(config, "POST", `/incoming-peppol-documents/${documentUlid}/confirm`, {});
  return { success: true };
}

async function pushBusinessCard(config: Config, input: any) {
  const { peppolId } = input;
  if (!peppolId) throw new Error("peppolId is required");
  await dokapiRequest(config, "POST", "/participant-registrations/business-cards/push", {
    scheme: "iso6523-actorid-upis", value: peppolId,
  });
  return { success: true };
}

function generateInvoiceResponseXml(
  id: string, sender: { peppolId: string; name: string }, receiver: { peppolId: string; name: string },
  invoiceRef: { number: string; date?: string }, reasonCode: string, reason: string,
): string {
  const today = new Date().toISOString().slice(0, 10);
  const esc = escapeXml;
  const senderScheme = sender.peppolId.split(":")[0];
  const senderValue = sender.peppolId.split(":").slice(1).join(":");
  const receiverScheme = receiver.peppolId.split(":")[0];
  const receiverValue = receiver.peppolId.split(":").slice(1).join(":");
  return `<?xml version="1.0" encoding="UTF-8"?>
<ApplicationResponse xmlns="urn:oasis:names:specification:ubl:schema:xsd:ApplicationResponse-2"
  xmlns:cac="urn:oasis:names:specification:ubl:schema:xsd:CommonAggregateComponents-2"
  xmlns:cbc="urn:oasis:names:specification:ubl:schema:xsd:CommonBasicComponents-2">
  <cbc:CustomizationID>urn:fdc:peppol.eu:poacc:trns:invoice_response:3</cbc:CustomizationID>
  <cbc:ProfileID>urn:fdc:peppol.eu:poacc:bis:invoice_response:3</cbc:ProfileID>
  <cbc:ID>${esc(id)}</cbc:ID>
  <cbc:IssueDate>${today}</cbc:IssueDate>
  <cbc:Note>${esc(reason)}</cbc:Note>
  <cac:SenderParty>
    <cbc:EndpointID schemeID="${senderScheme}">${esc(senderValue)}</cbc:EndpointID>
    <cac:PartyLegalEntity><cbc:RegistrationName>${esc(sender.name)}</cbc:RegistrationName></cac:PartyLegalEntity>
  </cac:SenderParty>
  <cac:ReceiverParty>
    <cbc:EndpointID schemeID="${receiverScheme}">${esc(receiverValue)}</cbc:EndpointID>
    <cac:PartyLegalEntity><cbc:RegistrationName>${esc(receiver.name)}</cbc:RegistrationName></cac:PartyLegalEntity>
  </cac:ReceiverParty>
  <cac:DocumentResponse>
    <cac:Response>
      <cbc:ResponseCode>RE</cbc:ResponseCode>
      <cbc:EffectiveDate>${today}</cbc:EffectiveDate>
      <cac:Status>
        <cbc:StatusReasonCode listID="OPStatusReason">${esc(reasonCode)}</cbc:StatusReasonCode>
        <cbc:StatusReason>${esc(reason)}</cbc:StatusReason>
      </cac:Status>
    </cac:Response>
    <cac:DocumentReference>
      <cbc:ID>${esc(invoiceRef.number)}</cbc:ID>${invoiceRef.date ? `\n      <cbc:IssueDate>${esc(invoiceRef.date)}</cbc:IssueDate>` : ""}
      <cbc:DocumentTypeCode>380</cbc:DocumentTypeCode>
    </cac:DocumentReference>
  </cac:DocumentResponse>
</ApplicationResponse>`;
}

async function rejectInvoice(config: Config, input: any) {
  const { senderPeppolId, senderName, receiverPeppolId, receiverName,
          originalInvoiceNumber, originalInvoiceDate, countryCode = "BE",
          reason = "Invoice rejected", reasonCode = "OTH" } = input;
  for (const [k, v] of [["senderPeppolId", senderPeppolId], ["senderName", senderName],
    ["receiverPeppolId", receiverPeppolId], ["receiverName", receiverName],
    ["originalInvoiceNumber", originalInvoiceNumber]] as const)
    if (!v) throw new Error(`${k} is required`);

  const responseId = `IMR-${Date.now()}`;
  const xml = generateInvoiceResponseXml(responseId,
    { peppolId: senderPeppolId, name: senderName },
    { peppolId: receiverPeppolId, name: receiverName },
    { number: originalInvoiceNumber, date: originalInvoiceDate }, reasonCode, reason);

  const peppolParticipant = (id: string) => {
    const [scheme, raw] = id.split(":");
    return { scheme: "iso6523-actorid-upis", value: `${scheme}:${extractIdentifier(raw || "")}` };
  };

  const res = await dokapiRequest<any>(config, "POST", "/outgoing-peppol-documents", {
    c1CountryCode: countryCode.toUpperCase(),
    sender: peppolParticipant(senderPeppolId),
    receiver: peppolParticipant(receiverPeppolId),
    documentTypeIdentifier: { scheme: "busdox-docid-qns", value: BIS3_IMR_DOCTYPE },
    processIdentifier: { scheme: "cenbii-procid-ubl", value: BIS3_IMR_PROCESS },
  });

  const upload = await fetch(res.preSignedUploadUrl, { method: "PUT", headers: { "Content-Type": "application/xml" }, body: xml });
  if (!upload.ok) throw new Error(`Upload failed: ${upload.status}`);

  return { dokapiUlid: res.document.ulid, responseId, status: res.document.status || "sending" };
}

// ─── Action Dispatch ─────────────────────────────────────────────────────────

const actions: Record<string, (config: Config, input: any) => Promise<any> | any> = {
  register_participant: registerParticipant,
  deregister_participant: deregisterParticipant,
  register_document_types: registerDocumentTypes,
  refresh_participant_status: refreshParticipantStatus,
  send_invoice: sendInvoice,
  send_test_invoice: sendTestInvoice,
  validate_document: validateDocument,
  generate_ubl: generateUbl,
  parse_ubl: parseUblAction,
  list_webhooks: listWebhooks,
  register_webhook: registerWebhook,
  delete_webhook: deleteWebhook,
  download_document: downloadDocument,
  confirm_document_download: confirmDocumentDownload,
  push_business_card: pushBusinessCard,
  reject_invoice: rejectInvoice,
};

// ─── Webhook Handler ─────────────────────────────────────────────────────────

async function handleWebhook(params: any) {
  const deps: WebhookDeps = {
    collectionOp,
    uploadFile: (globalThis as any).uploadFile,
    dokapiRequest,
  };
  return handleWebhookImpl(params, deps);
}

// ─── IPC Protocol ────────────────────────────────────────────────────────────

const rpcHandlers: Record<string, (params: any) => Promise<any>> = {
  async __bind(params) {
    const { config } = params;
    if (!config?.clientId || !config?.clientSecret) return {};
    const secret = await dokapiRequest<string>(config, "POST", "/webhooks/secretKey");
    return { mergeConfig: { webhookSecret: secret } };
  },

  async __integration(params) {
    const { action, input, config } = params;
    if (!config?.baseUrl && (!config?.clientId || !config?.clientSecret))
      throw new Error("Dokapi credentials or baseUrl not configured");
    const handler = actions[action];
    if (!handler) throw new Error(`unknown action: ${action}`);
    return handler(config, input);
  },

  async __webhook(params) {
    return handleWebhook(params);
  },
};

const send = (msg: Record<string, unknown>) => process.stdout.write(JSON.stringify(msg) + "\n");
const pendingOps = new Map<string, { resolve: (v: any) => void; reject: (e: Error) => void }>();
let opSeq = 0;

function collectionOp(op: string, entity: string, data: Record<string, unknown>): Promise<any> {
  const id = `cop_${++opSeq}`;
  return new Promise((resolve, reject) => {
    pendingOps.set(id, { resolve, reject });
    send({ type: "collection_op", id, op, entity, data });
  });
}

let buffer = "";
process.stdin.setEncoding("utf-8");
process.stdin.on("data", (chunk: string) => {
  buffer += chunk;
  let nl: number;
  while ((nl = buffer.indexOf("\n")) !== -1) {
    const line = buffer.slice(0, nl).trim();
    buffer = buffer.slice(nl + 1);
    if (!line) continue;
    const msg = JSON.parse(line);
    switch (msg.type) {
      case "discover":
        send({ type: "discover", methods: Object.keys(rpcHandlers) });
        break;
      case "rpc":
        handleRpc(msg);
        break;
      case "collection_op_result": {
        const p = pendingOps.get(msg.id);
        if (!p) break;
        pendingOps.delete(msg.id);
        msg.error ? p.reject(new Error(msg.error)) : p.resolve(msg.result);
        break;
      }
      case "shutdown":
        process.exit(0);
    }
  }
});

async function handleRpc(msg: any) {
  const handler = rpcHandlers[msg.method];
  if (!handler) {
    send({ type: "rpc_response", id: msg.id, error: `unknown method: ${msg.method}` });
    return;
  }
  try {
    send({ type: "rpc_response", id: msg.id, result: await handler(msg.params) });
  } catch (e: any) {
    send({ type: "rpc_response", id: msg.id, error: e.message });
  }
}
