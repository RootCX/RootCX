export interface InvoiceParty {
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

export interface InvoiceLine {
  id: string;
  description: string;
  quantity: number;
  unitCode?: string;
  unitPrice: number;
  taxPercent: number;
  taxCategory?: string;
  lineAmount: number;
}

export interface PaymentInfo {
  iban: string;
  bic?: string;
  bankName?: string;
  accountName?: string;
}

export interface EmbeddedAttachment {
  base64Content: string;
  mimeCode: string;
  filename: string;
}

export interface DocumentReference {
  id: string;
  typeCode?: string;
  description?: string;
  attachment?: EmbeddedAttachment;
}

export interface InvoiceParams {
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

// Generic Peppol BIS Billing 3.0 / UBL 2.1 CreditNote (CreditNoteTypeCode 381).
// Country-agnostic, exactly like InvoiceParams. Any statutory wording (e.g. a
// VAT-reversal mention) is supplied by the caller via `note` — nothing is
// hardcoded per jurisdiction. The mandatory BillingReference points at the
// invoice being corrected (EN16931 BG-3 / BT-25), which is what ties the credit
// note to its original invoice on any Peppol network.
export interface CreditNoteParams {
  creditNoteNumber: string;
  issueDate: string;
  currency?: string;
  /** Number of the invoice this credit note corrects/cancels (EN16931 BT-25). Required. */
  correctedInvoiceNumber: string;
  /** Issue date of the corrected invoice (EN16931 BT-26), YYYY-MM-DD. */
  correctedInvoiceDate?: string;
  buyerReference?: string;
  orderReference?: string;
  contractReference?: string;
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
  /** Free-text payment terms; emitted to satisfy EN16931 BR-CO-25 (positive payable amount needs terms or due date). */
  paymentTermsNote?: string;
}

export function escapeXml(str: string): string {
  return str.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;").replace(/'/g, "&apos;");
}

export function extractIdentifier(peppolId: string): string {
  const value = peppolId.split(":")[1] || peppolId;
  return value.replace(/^[A-Z]{2}/i, "");
}

function formatVat(vatNumber: string, countryCode: string): string {
  const clean = vatNumber.replace(/[^0-9A-Z]/gi, "").toUpperCase();
  return clean.match(/^[A-Z]{2}/) ? clean : `${countryCode}${clean}`;
}

// ─── Shared UBL builders ───────────────────────────────────────────────────────
//
// Invoice and CreditNote share all of their party/tax/total/reference markup;
// only the root element, type code, line element names and a few document-level
// references differ. These helpers produce the common substrings so both
// generators stay in lock-step (one place to fix any EN16931 rule change).

const resolveTaxCategory = (cat: string | undefined, pct: number) => cat || (pct === 0 ? "E" : "S");

interface TaxSubtotal { category: string; percent: number; taxableAmount: number; taxAmount: number; }

// EN16931: BR-CO-14 (TaxTotal = sum of subtotals), BR-CO-15 (Inclusive = Exclusive + Tax).
function computeTaxSummary(lines: InvoiceLine[], taxableAmount: number): {
  taxSubtotals: TaxSubtotal[]; taxTotal: number; taxInclusiveAmount: number;
} {
  // Group lines by tax category + rate for the TaxSubtotal breakdown.
  const taxGroups = lines.reduce((m, l) => {
    const cat = resolveTaxCategory(l.taxCategory, l.taxPercent), pct = l.taxPercent ?? 21, key = `${cat}:${pct}`;
    const g = m.get(key) || { category: cat, percent: pct, taxableAmount: 0 };
    g.taxableAmount += l.lineAmount;
    return m.set(key, g);
  }, new Map<string, { category: string; percent: number; taxableAmount: number }>());

  const taxSubtotals: TaxSubtotal[] = Array.from(taxGroups.values()).map(g => ({
    ...g,
    taxAmount: Math.round(g.taxableAmount * g.percent) / 100,
  }));
  const taxTotal = taxSubtotals.reduce((s, g) => s + g.taxAmount, 0);
  const taxInclusiveAmount = Math.round((taxableAmount + taxTotal) * 100) / 100;
  return { taxSubtotals, taxTotal, taxInclusiveAmount };
}

const noteEl = (note?: string) =>
  note ? `\n    <cbc:Note>${escapeXml(note)}</cbc:Note>` : "";
const orderRefEl = (ref?: string) =>
  ref ? `\n    <cac:OrderReference><cbc:ID>${escapeXml(ref)}</cbc:ID></cac:OrderReference>` : "";
const contractRefEl = (ref?: string) =>
  ref ? `\n    <cac:ContractDocumentReference><cbc:ID>${escapeXml(ref)}</cbc:ID></cac:ContractDocumentReference>` : "";
const projectRefEl = (ref?: string) =>
  ref ? `\n    <cac:ProjectReference><cbc:ID>${escapeXml(ref)}</cbc:ID></cac:ProjectReference>` : "";
const originatorRefEl = (ref?: string) =>
  ref ? `\n    <cac:OriginatorDocumentReference><cbc:ID>${escapeXml(ref)}</cbc:ID></cac:OriginatorDocumentReference>` : "";

const docRefEls = (refs?: DocumentReference[]) =>
  refs?.map((ref) => `
    <cac:AdditionalDocumentReference>
        <cbc:ID>${escapeXml(ref.id)}</cbc:ID>${ref.typeCode ? `\n        <cbc:DocumentTypeCode>${escapeXml(ref.typeCode)}</cbc:DocumentTypeCode>` : ""}${ref.description ? `\n        <cbc:DocumentDescription>${escapeXml(ref.description)}</cbc:DocumentDescription>` : ""}${ref.attachment ? `
        <cac:Attachment>
            <cbc:EmbeddedDocumentBinaryObject mimeCode="${ref.attachment.mimeCode}" filename="${escapeXml(ref.attachment.filename)}">${ref.attachment.base64Content}</cbc:EmbeddedDocumentBinaryObject>
        </cac:Attachment>` : ""}
    </cac:AdditionalDocumentReference>`).join("") || "";

const paymentMeansEl = (paymentInfo: PaymentInfo | undefined, paymentId: string) =>
  paymentInfo?.iban ? `
    <cac:PaymentMeans>
        <cbc:PaymentMeansCode>30</cbc:PaymentMeansCode>
        <cbc:PaymentID>${escapeXml(paymentId)}</cbc:PaymentID>
        <cac:PayeeFinancialAccount>
            <cbc:ID>${escapeXml(paymentInfo.iban)}</cbc:ID>${paymentInfo.accountName ? `\n            <cbc:Name>${escapeXml(paymentInfo.accountName)}</cbc:Name>` : ""}${paymentInfo.bic ? `
            <cac:FinancialInstitutionBranch>
                <cbc:ID>${escapeXml(paymentInfo.bic)}</cbc:ID>
            </cac:FinancialInstitutionBranch>` : ""}
        </cac:PayeeFinancialAccount>
    </cac:PaymentMeans>` : "";

function supplierPartyEl(supplier: InvoiceParty): string {
  const supplierId = extractIdentifier(supplier.peppolId);
  const supplierVat = formatVat(supplier.vatNumber, supplier.countryCode);
  const schemeId = supplier.peppolId.split(":")[0] || "0208";
  return `<cac:AccountingSupplierParty>
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
    </cac:AccountingSupplierParty>`;
}

function customerPartyEl(customer: InvoiceParty, schemeId: string): string {
  const customerId = extractIdentifier(customer.peppolId);
  const customerVat = formatVat(customer.vatNumber, customer.countryCode);
  const customerContactEl = (customer.contactName || customer.contactEmail || customer.contactPhone)
    ? `\n            <cac:Contact>${customer.contactName ? `\n                <cbc:Name>${escapeXml(customer.contactName)}</cbc:Name>` : ""}${customer.contactPhone ? `\n                <cbc:Telephone>${escapeXml(customer.contactPhone)}</cbc:Telephone>` : ""}${customer.contactEmail ? `\n                <cbc:ElectronicMail>${escapeXml(customer.contactEmail)}</cbc:ElectronicMail>` : ""}\n            </cac:Contact>` : "";
  return `<cac:AccountingCustomerParty>
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
    </cac:AccountingCustomerParty>`;
}

function taxTotalEl(taxSubtotals: TaxSubtotal[], taxTotal: number, currency: string): string {
  return `<cac:TaxTotal>
        <cbc:TaxAmount currencyID="${currency}">${taxTotal.toFixed(2)}</cbc:TaxAmount>${taxSubtotals.map(g => `
        <cac:TaxSubtotal>
            <cbc:TaxableAmount currencyID="${currency}">${g.taxableAmount.toFixed(2)}</cbc:TaxableAmount>
            <cbc:TaxAmount currencyID="${currency}">${g.taxAmount.toFixed(2)}</cbc:TaxAmount>
            <cac:TaxCategory>
                <cbc:ID>${g.category}</cbc:ID>
                <cbc:Percent>${g.percent}</cbc:Percent>${g.category === "E" ? `
                <cbc:TaxExemptionReasonCode>vatex-eu-132</cbc:TaxExemptionReasonCode>
                <cbc:TaxExemptionReason>Exempt</cbc:TaxExemptionReason>` : ""}
                <cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme>
            </cac:TaxCategory>
        </cac:TaxSubtotal>`).join("")}
    </cac:TaxTotal>`;
}

function legalMonetaryTotalEl(taxableAmount: number, taxInclusiveAmount: number, currency: string): string {
  return `<cac:LegalMonetaryTotal>
        <cbc:LineExtensionAmount currencyID="${currency}">${taxableAmount.toFixed(2)}</cbc:LineExtensionAmount>
        <cbc:TaxExclusiveAmount currencyID="${currency}">${taxableAmount.toFixed(2)}</cbc:TaxExclusiveAmount>
        <cbc:TaxInclusiveAmount currencyID="${currency}">${taxInclusiveAmount.toFixed(2)}</cbc:TaxInclusiveAmount>
        <cbc:PayableAmount currencyID="${currency}">${taxInclusiveAmount.toFixed(2)}</cbc:PayableAmount>
    </cac:LegalMonetaryTotal>`;
}

// Renders document lines. `lineTag`/`qtyTag` differ between Invoice
// (InvoiceLine / InvoicedQuantity) and CreditNote (CreditNoteLine / CreditedQuantity).
function documentLinesEl(lines: InvoiceLine[], currency: string, lineTag: string, qtyTag: string): string {
  return lines.map((l) => `
    <cac:${lineTag}>
        <cbc:ID>${escapeXml(l.id)}</cbc:ID>
        <cbc:${qtyTag} unitCode="${l.unitCode || "C62"}">${l.quantity}</cbc:${qtyTag}>
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
    </cac:${lineTag}>`).join("");
}

// ─── Invoice ───────────────────────────────────────────────────────────────────

export function generateInvoiceXml(params: InvoiceParams): string {
  const {
    invoiceNumber, issueDate, dueDate, currency = "EUR",
    buyerReference = invoiceNumber, supplier, customer, lines, taxableAmount, note,
  } = params;

  const schemeId = supplier.peppolId.split(":")[0] || "0208";
  const { taxSubtotals, taxTotal, taxInclusiveAmount } = computeTaxSummary(lines, taxableAmount);

  return `<?xml version="1.0" encoding="UTF-8"?>
<Invoice xmlns="urn:oasis:names:specification:ubl:schema:xsd:Invoice-2"
         xmlns:cac="urn:oasis:names:specification:ubl:schema:xsd:CommonAggregateComponents-2"
         xmlns:cbc="urn:oasis:names:specification:ubl:schema:xsd:CommonBasicComponents-2">
    <cbc:CustomizationID>urn:cen.eu:en16931:2017#compliant#urn:fdc:peppol.eu:2017:poacc:billing:3.0</cbc:CustomizationID>
    <cbc:ProfileID>urn:fdc:peppol.eu:2017:poacc:billing:01:1.0</cbc:ProfileID>
    <cbc:ID>${escapeXml(invoiceNumber)}</cbc:ID>
    <cbc:IssueDate>${issueDate}</cbc:IssueDate>
    <cbc:DueDate>${dueDate}</cbc:DueDate>
    <cbc:InvoiceTypeCode>380</cbc:InvoiceTypeCode>${noteEl(note)}
    <cbc:DocumentCurrencyCode>${currency}</cbc:DocumentCurrencyCode>
    <cbc:BuyerReference>${escapeXml(buyerReference)}</cbc:BuyerReference>${orderRefEl(params.orderReference)}${originatorRefEl(params.originatorReference)}${contractRefEl(params.contractReference)}${docRefEls(params.documentReferences)}${projectRefEl(params.projectReference)}
    ${supplierPartyEl(supplier)}
    ${customerPartyEl(customer, schemeId)}${paymentMeansEl(params.paymentInfo, invoiceNumber)}
    ${taxTotalEl(taxSubtotals, taxTotal, currency)}
    ${legalMonetaryTotalEl(taxableAmount, taxInclusiveAmount, currency)}${documentLinesEl(lines, currency, "InvoiceLine", "InvoicedQuantity")}
</Invoice>`;
}

// ─── Credit Note ───────────────────────────────────────────────────────────────

export function generateCreditNoteXml(params: CreditNoteParams): string {
  const {
    creditNoteNumber, issueDate, currency = "EUR",
    correctedInvoiceNumber, correctedInvoiceDate,
    buyerReference = creditNoteNumber, supplier, customer, lines, taxableAmount, note,
    paymentTermsNote = "Credit note relating to the referenced invoice.",
  } = params;

  if (!correctedInvoiceNumber) throw new Error("correctedInvoiceNumber is required for a credit note");

  const schemeId = supplier.peppolId.split(":")[0] || "0208";
  const { taxSubtotals, taxTotal, taxInclusiveAmount } = computeTaxSummary(lines, taxableAmount);

  const billingRefEl = `
    <cac:BillingReference>
        <cac:InvoiceDocumentReference>
            <cbc:ID>${escapeXml(correctedInvoiceNumber)}</cbc:ID>${correctedInvoiceDate ? `\n            <cbc:IssueDate>${correctedInvoiceDate}</cbc:IssueDate>` : ""}
        </cac:InvoiceDocumentReference>
    </cac:BillingReference>`;

  return `<?xml version="1.0" encoding="UTF-8"?>
<CreditNote xmlns="urn:oasis:names:specification:ubl:schema:xsd:CreditNote-2"
         xmlns:cac="urn:oasis:names:specification:ubl:schema:xsd:CommonAggregateComponents-2"
         xmlns:cbc="urn:oasis:names:specification:ubl:schema:xsd:CommonBasicComponents-2">
    <cbc:CustomizationID>urn:cen.eu:en16931:2017#compliant#urn:fdc:peppol.eu:2017:poacc:billing:3.0</cbc:CustomizationID>
    <cbc:ProfileID>urn:fdc:peppol.eu:2017:poacc:billing:01:1.0</cbc:ProfileID>
    <cbc:ID>${escapeXml(creditNoteNumber)}</cbc:ID>
    <cbc:IssueDate>${issueDate}</cbc:IssueDate>
    <cbc:CreditNoteTypeCode>381</cbc:CreditNoteTypeCode>${noteEl(note)}
    <cbc:DocumentCurrencyCode>${currency}</cbc:DocumentCurrencyCode>
    <cbc:BuyerReference>${escapeXml(buyerReference)}</cbc:BuyerReference>${orderRefEl(params.orderReference)}${billingRefEl}${contractRefEl(params.contractReference)}${docRefEls(params.documentReferences)}${originatorRefEl(params.originatorReference)}
    ${supplierPartyEl(supplier)}
    ${customerPartyEl(customer, schemeId)}${paymentMeansEl(params.paymentInfo, creditNoteNumber)}
    <cac:PaymentTerms>
        <cbc:Note>${escapeXml(paymentTermsNote)}</cbc:Note>
    </cac:PaymentTerms>
    ${taxTotalEl(taxSubtotals, taxTotal, currency)}
    ${legalMonetaryTotalEl(taxableAmount, taxInclusiveAmount, currency)}${documentLinesEl(lines, currency, "CreditNoteLine", "CreditedQuantity")}
</CreditNote>`;
}
