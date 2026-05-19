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

export function generateInvoiceXml(params: InvoiceParams): string {
  const {
    invoiceNumber, issueDate, dueDate, currency = "EUR",
    buyerReference = invoiceNumber, orderReference, supplier, customer,
    lines, taxableAmount, note,
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

  // EN16931: BR-CO-14 (TaxTotal = sum of subtotals), BR-CO-15 (Inclusive = Exclusive + Tax)
  const taxSubtotals = Array.from(taxGroups.values()).map(g => ({
    ...g,
    taxAmount: Math.round(g.taxableAmount * g.percent) / 100,
  }));
  const taxTotal = taxSubtotals.reduce((s, g) => s + g.taxAmount, 0);
  const taxInclusiveAmount = Math.round((taxableAmount + taxTotal) * 100) / 100;

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
    </cac:TaxTotal>
    <cac:LegalMonetaryTotal>
        <cbc:LineExtensionAmount currencyID="${currency}">${taxableAmount.toFixed(2)}</cbc:LineExtensionAmount>
        <cbc:TaxExclusiveAmount currencyID="${currency}">${taxableAmount.toFixed(2)}</cbc:TaxExclusiveAmount>
        <cbc:TaxInclusiveAmount currencyID="${currency}">${taxInclusiveAmount.toFixed(2)}</cbc:TaxInclusiveAmount>
        <cbc:PayableAmount currencyID="${currency}">${taxInclusiveAmount.toFixed(2)}</cbc:PayableAmount>
    </cac:LegalMonetaryTotal>${invoiceLines}
</Invoice>`;
}
