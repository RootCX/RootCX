import { XMLParser } from "fast-xml-parser";

// ─── Types ──────────────────────────────────────────────────────────────────

export interface ParsedAddress {
  street?: string;
  additionalStreet?: string;
  city?: string;
  postalZone?: string;
  countrySubentity?: string;
  addressLine?: string;
  countryCode?: string;
}

export interface ParsedContact {
  name?: string;
  phone?: string;
  email?: string;
}

export interface ParsedParty {
  peppolId: string;
  name: string;
  vatNumber?: string;
  companyId?: string;
  companyLegalForm?: string;
  address?: ParsedAddress;
  contact?: ParsedContact;
}

export interface ParsedTaxSubtotal {
  taxableAmount: number;
  taxAmount: number;
  category: string;
  percent: number;
  exemptionReasonCode?: string;
  exemptionReason?: string;
}

export interface ParsedMonetaryTotal {
  lineExtensionAmount: number;
  taxExclusiveAmount: number;
  taxInclusiveAmount: number;
  payableAmount: number;
  allowanceTotalAmount?: number;
  chargeTotalAmount?: number;
  prepaidAmount?: number;
  payableRoundingAmount?: number;
}

export interface ParsedPeriod {
  startDate?: string;
  endDate?: string;
  descriptionCode?: string;
}

export interface ParsedAllowanceCharge {
  chargeIndicator: boolean;
  reasonCode?: string;
  reason?: string;
  multiplier?: number;
  amount: number;
  baseAmount?: number;
  taxCategory?: string;
  taxPercent?: number;
}

export interface ParsedInvoiceLine {
  id: string;
  note?: string;
  quantity: number;
  unitCode: string;
  lineAmount: number;
  accountingCost?: string;
  period?: ParsedPeriod;
  orderLineReference?: string;
  documentReference?: { id: string; typeCode?: string };
  allowanceCharges?: ParsedAllowanceCharge[];
  description: string;
  itemDescription?: string;
  buyersItemId?: string;
  sellersItemId?: string;
  standardItemId?: string;
  originCountry?: string;
  commodityClassifications?: { code: string; listId: string }[];
  additionalProperties?: { name: string; value: string }[];
  unitPrice: number;
  baseQuantity?: number;
  priceAllowance?: { amount: number; baseAmount?: number };
  taxCategory: string;
  taxPercent: number;
}

export interface ParsedPaymentMeans {
  code: string;
  paymentId?: string;
  iban?: string;
  accountName?: string;
  bic?: string;
  card?: { accountNumber: string; network: string; holderName?: string };
  mandate?: { id?: string; payerAccount?: string };
}

export interface ParsedBillingReference {
  id: string;
  issueDate?: string;
}

export interface ParsedDelivery {
  date?: string;
  locationId?: string;
  address?: ParsedAddress;
  partyName?: string;
}

export interface ParsedPayeeParty {
  name: string;
  companyId?: string;
}

export interface ParsedTaxRepParty {
  name: string;
  vatNumber?: string;
  address?: ParsedAddress;
}

export interface ParsedUBLDocument {
  customizationId: string;
  profileId: string;
  documentNumber: string;
  documentType: "invoice" | "credit_note";
  typeCode: string;
  issueDate: string;
  dueDate?: string;
  note?: string;
  taxPointDate?: string;
  taxCurrencyCode?: string;
  accountingCost?: string;
  currency: string;
  buyerReference?: string;
  invoicePeriod?: ParsedPeriod;
  orderReference?: string;
  salesOrderId?: string;
  billingReferences?: ParsedBillingReference[];
  despatchDocumentReference?: string;
  receiptDocumentReference?: string;
  originatorDocumentReference?: string;
  contractReference?: string;
  projectReference?: string;
  seller: ParsedParty;
  buyer: ParsedParty;
  payeeParty?: ParsedPayeeParty;
  taxRepresentativeParty?: ParsedTaxRepParty;
  delivery?: ParsedDelivery;
  monetaryTotal: ParsedMonetaryTotal;
  taxTotal: { taxAmount: number; subtotals: ParsedTaxSubtotal[] };
  taxCurrencyTotal?: number;
  lines: ParsedInvoiceLine[];
  paymentMeans?: ParsedPaymentMeans[];
  paymentTerms?: string;
  allowanceCharges?: ParsedAllowanceCharge[];
  instanceIdentifier?: string;
  attachments: { id: string; description?: string; mimeCode: string; filename: string; base64Content: string }[];
}

// ─── XML Helpers ────────────────────────────────────────────────────────────

const xmlParser = new XMLParser({
  ignoreAttributes: false,
  attributeNamePrefix: "@_",
  removeNSPrefix: true,
  parseTagValue: false,
  isArray: (name) =>
    ["InvoiceLine", "CreditNoteLine", "AdditionalDocumentReference", "DocumentResponse",
     "TaxSubtotal", "PaymentMeans", "AllowanceCharge", "BillingReference", "PartyIdentification",
     "CommodityClassification", "AdditionalItemProperty", "TaxTotal"].includes(name),
});

function txt(node: unknown): string {
  if (typeof node === "string") return node;
  if (typeof node === "number") return String(node);
  if (node && typeof node === "object" && "#text" in node) return String((node as any)["#text"]);
  return "";
}

function get(obj: unknown, path: string): unknown {
  let cur = obj;
  for (const key of path.split(".")) {
    if (cur && typeof cur === "object" && key in cur) cur = (cur as any)[key];
    else return undefined;
  }
  return cur;
}

function num(node: unknown): number {
  const v = parseFloat(txt(node));
  return isNaN(v) ? 0 : v;
}

function optTxt(obj: unknown, path: string): string | undefined {
  return txt(get(obj, path)) || undefined;
}

function optNum(obj: unknown, path: string): number | undefined {
  const node = get(obj, path);
  if (node === undefined) return undefined;
  const v = parseFloat(txt(node));
  return isNaN(v) ? undefined : v;
}

/** Assign non-undefined values to result object */
function setOpt<T extends Record<string, unknown>>(result: T, fields: Record<string, unknown>): void {
  for (const [k, v] of Object.entries(fields)) {
    if (v !== undefined) (result as any)[k] = v;
  }
}

function asArray(val: unknown): unknown[] | undefined {
  return Array.isArray(val) && val.length > 0 ? val : undefined;
}

// ─── Extractors ─────────────────────────────────────────────────────────────

function extractAddress(container: unknown, key = "PostalAddress"): ParsedAddress | undefined {
  const addr = get(container, key);
  if (!addr) return undefined;
  const result: ParsedAddress = {};
  setOpt(result, {
    street: optTxt(addr, "StreetName"),
    additionalStreet: optTxt(addr, "AdditionalStreetName"),
    city: optTxt(addr, "CityName"),
    postalZone: optTxt(addr, "PostalZone"),
    countrySubentity: optTxt(addr, "CountrySubentity"),
    addressLine: optTxt(addr, "AddressLine.Line"),
    countryCode: optTxt(addr, "Country.IdentificationCode"),
  });
  return Object.keys(result).length ? result : undefined;
}

function extractPeppolId(party: unknown): string {
  const endpointId = get(party, "EndpointID");
  if (endpointId) {
    const scheme = txt(get(endpointId, "@_schemeID")) || "";
    const value = txt(endpointId);
    return scheme && value ? `${scheme}:${value}` : value;
  }
  const ids = asArray(get(party, "PartyIdentification"));
  if (ids) {
    const id = get(ids[0], "ID");
    const scheme = txt(get(id, "@_schemeID")) || "";
    const value = txt(id);
    return scheme && value ? `${scheme}:${value}` : value;
  }
  return "";
}

function extractVatNumber(party: unknown): string | undefined {
  const tax = get(party, "PartyTaxScheme.CompanyID");
  if (tax) return txt(tax);
  // PartyTaxScheme can be array (0..2 for supplier)
  const taxArr = asArray(get(party, "PartyTaxScheme"));
  if (taxArr) {
    for (const t of taxArr) {
      const v = optTxt(t, "CompanyID");
      if (v) return v;
    }
  }
  const legal = get(party, "PartyLegalEntity.CompanyID");
  if (legal) return txt(legal);
  return undefined;
}

function extractContact(party: unknown): ParsedContact | undefined {
  const c = get(party, "Contact");
  if (!c) return undefined;
  const result: ParsedContact = {};
  setOpt(result, { name: optTxt(c, "Name"), phone: optTxt(c, "Telephone"), email: optTxt(c, "ElectronicMail") });
  return Object.keys(result).length ? result : undefined;
}

function extractParty(party: unknown): ParsedParty {
  const result: ParsedParty = {
    peppolId: extractPeppolId(party),
    name: txt(get(party, "PartyName.Name")) || txt(get(party, "PartyLegalEntity.RegistrationName")) || "",
    vatNumber: extractVatNumber(party),
  };
  setOpt(result, {
    companyId: optTxt(party, "PartyLegalEntity.CompanyID"),
    companyLegalForm: optTxt(party, "PartyLegalEntity.CompanyLegalForm"),
    address: extractAddress(party),
    contact: extractContact(party),
  });
  return result;
}

function extractPeriod(container: unknown, key = "InvoicePeriod"): ParsedPeriod | undefined {
  const p = get(container, key);
  if (!p) return undefined;
  const result: ParsedPeriod = {};
  setOpt(result, { startDate: optTxt(p, "StartDate"), endDate: optTxt(p, "EndDate"), descriptionCode: optTxt(p, "DescriptionCode") });
  return Object.keys(result).length ? result : undefined;
}

function extractAllowanceCharge(ac: unknown): ParsedAllowanceCharge {
  const result: ParsedAllowanceCharge = {
    chargeIndicator: txt(get(ac, "ChargeIndicator")) === "true",
    amount: num(get(ac, "Amount")),
  };
  setOpt(result, {
    reasonCode: optTxt(ac, "AllowanceChargeReasonCode"),
    reason: optTxt(ac, "AllowanceChargeReason"),
    multiplier: optNum(ac, "MultiplierFactorNumeric"),
    baseAmount: optNum(ac, "BaseAmount"),
    taxCategory: optTxt(ac, "TaxCategory.ID"),
    taxPercent: optNum(ac, "TaxCategory.Percent"),
  });
  return result;
}

function extractLines(doc: unknown, type: "invoice" | "credit_note"): ParsedInvoiceLine[] {
  const key = type === "invoice" ? "InvoiceLine" : "CreditNoteLine";
  const qtyKey = type === "invoice" ? "InvoicedQuantity" : "CreditedQuantity";
  const raw = asArray(get(doc, key));
  if (!raw) return [];
  return raw.map((line) => {
    const qtyNode = get(line, qtyKey);
    const result: ParsedInvoiceLine = {
      id: txt(get(line, "ID")),
      quantity: num(qtyNode),
      unitCode: txt(get(qtyNode, "@_unitCode")) || "C62",
      lineAmount: num(get(line, "LineExtensionAmount")),
      description: txt(get(line, "Item.Name")),
      unitPrice: num(get(line, "Price.PriceAmount")),
      taxCategory: txt(get(line, "Item.ClassifiedTaxCategory.ID")),
      taxPercent: num(get(line, "Item.ClassifiedTaxCategory.Percent")),
    };

    setOpt(result, {
      note: optTxt(line, "Note"),
      accountingCost: optTxt(line, "AccountingCost"),
      period: extractPeriod(line),
      orderLineReference: optTxt(line, "OrderLineReference.LineID"),
      itemDescription: optTxt(line, "Item.Description"),
      buyersItemId: optTxt(line, "Item.BuyersItemIdentification.ID"),
      sellersItemId: optTxt(line, "Item.SellersItemIdentification.ID"),
      standardItemId: optTxt(line, "Item.StandardItemIdentification.ID"),
      originCountry: optTxt(line, "Item.OriginCountry.IdentificationCode"),
      baseQuantity: optNum(line, "Price.BaseQuantity"),
    });

    const drId = optTxt(line, "DocumentReference.ID");
    if (drId) {
      const typeCode = optTxt(line, "DocumentReference.DocumentTypeCode");
      result.documentReference = typeCode ? { id: drId, typeCode } : { id: drId };
    }

    const acs = asArray(get(line, "AllowanceCharge"));
    if (acs) result.allowanceCharges = acs.map(extractAllowanceCharge);

    const ccRaw = asArray(get(line, "Item.CommodityClassification"));
    if (ccRaw) {
      result.commodityClassifications = ccRaw.map((cc) => {
        const node = get(cc, "ItemClassificationCode");
        return { code: txt(node), listId: txt(get(node, "@_listID")) };
      });
    }

    const props = asArray(get(line, "Item.AdditionalItemProperty"));
    if (props) result.additionalProperties = props.map((p) => ({ name: txt(get(p, "Name")), value: txt(get(p, "Value")) }));

    const priceAcArr = asArray(get(line, "Price.AllowanceCharge"));
    if (priceAcArr) {
      const pa: { amount: number; baseAmount?: number } = { amount: num(get(priceAcArr[0], "Amount")) };
      const ba = optNum(priceAcArr[0], "BaseAmount");
      if (ba !== undefined) pa.baseAmount = ba;
      result.priceAllowance = pa;
    }

    return result;
  });
}

function extractTaxTotals(doc: unknown): { primary: { taxAmount: number; subtotals: ParsedTaxSubtotal[] }; currencyTotal?: number } {
  const totals = asArray(get(doc, "TaxTotal"));
  if (!totals) return { primary: { taxAmount: 0, subtotals: [] } };

  let primary: { taxAmount: number; subtotals: ParsedTaxSubtotal[] } = { taxAmount: 0, subtotals: [] };
  let currencyTotal: number | undefined;

  for (const tt of totals) {
    const subs = asArray(get(tt, "TaxSubtotal"));
    if (subs) {
      primary = {
        taxAmount: num(get(tt, "TaxAmount")),
        subtotals: subs.map((s) => {
          const base: ParsedTaxSubtotal = {
            taxableAmount: num(get(s, "TaxableAmount")),
            taxAmount: num(get(s, "TaxAmount")),
            category: txt(get(s, "TaxCategory.ID")),
            percent: num(get(s, "TaxCategory.Percent")),
          };
          setOpt(base, {
            exemptionReasonCode: optTxt(s, "TaxCategory.TaxExemptionReasonCode"),
            exemptionReason: optTxt(s, "TaxCategory.TaxExemptionReason"),
          });
          return base;
        }),
      };
    } else {
      currencyTotal = num(get(tt, "TaxAmount"));
    }
  }

  return { primary, currencyTotal };
}

function extractMonetaryTotal(doc: unknown): ParsedMonetaryTotal {
  const mt = get(doc, "LegalMonetaryTotal");
  const result: ParsedMonetaryTotal = {
    lineExtensionAmount: num(get(mt, "LineExtensionAmount")),
    taxExclusiveAmount: num(get(mt, "TaxExclusiveAmount")),
    taxInclusiveAmount: num(get(mt, "TaxInclusiveAmount")),
    payableAmount: num(get(mt, "PayableAmount")),
  };
  setOpt(result, {
    allowanceTotalAmount: optNum(mt, "AllowanceTotalAmount"),
    chargeTotalAmount: optNum(mt, "ChargeTotalAmount"),
    prepaidAmount: optNum(mt, "PrepaidAmount"),
    payableRoundingAmount: optNum(mt, "PayableRoundingAmount"),
  });
  return result;
}

function extractPaymentMeans(doc: unknown): ParsedPaymentMeans[] | undefined {
  const raw = asArray(get(doc, "PaymentMeans"));
  if (!raw) return undefined;
  return raw.map((pm) => {
    const result: ParsedPaymentMeans = { code: txt(get(pm, "PaymentMeansCode")) };
    setOpt(result, {
      paymentId: optTxt(pm, "PaymentID"),
      iban: optTxt(pm, "PayeeFinancialAccount.ID"),
      accountName: optTxt(pm, "PayeeFinancialAccount.Name"),
      bic: optTxt(pm, "PayeeFinancialAccount.FinancialInstitutionBranch.ID"),
    });

    const cardNum = optTxt(pm, "CardAccount.PrimaryAccountNumberID");
    if (cardNum) {
      const card: ParsedPaymentMeans["card"] = { accountNumber: cardNum, network: txt(get(pm, "CardAccount.NetworkID")) };
      const holder = optTxt(pm, "CardAccount.HolderName");
      if (holder) card.holderName = holder;
      result.card = card;
    }

    const mandateId = optTxt(pm, "PaymentMandate.ID");
    const payerAccount = optTxt(pm, "PaymentMandate.PayerFinancialAccount.ID");
    if (mandateId || payerAccount) {
      const mandate: ParsedPaymentMeans["mandate"] = {};
      if (mandateId) mandate.id = mandateId;
      if (payerAccount) mandate.payerAccount = payerAccount;
      result.mandate = mandate;
    }

    return result;
  });
}

function extractBillingReferences(doc: unknown): ParsedBillingReference[] | undefined {
  const raw = asArray(get(doc, "BillingReference"));
  if (!raw) return undefined;
  return raw.map((br) => ({
    id: txt(get(br, "InvoiceDocumentReference.ID")),
    issueDate: optTxt(br, "InvoiceDocumentReference.IssueDate"),
  }));
}

function extractDelivery(doc: unknown): ParsedDelivery | undefined {
  const d = get(doc, "Delivery");
  if (!d) return undefined;
  const result: ParsedDelivery = {};
  setOpt(result, {
    date: optTxt(d, "ActualDeliveryDate"),
    locationId: optTxt(d, "DeliveryLocation.ID"),
    address: extractAddress(d, "DeliveryLocation.Address"),
    partyName: optTxt(d, "DeliveryParty.PartyName.Name"),
  });
  return Object.keys(result).length ? result : undefined;
}

function extractPayeeParty(doc: unknown): ParsedPayeeParty | undefined {
  const p = get(doc, "PayeeParty");
  if (!p) return undefined;
  const name = txt(get(p, "PartyName.Name"));
  if (!name) return undefined;
  const result: ParsedPayeeParty = { name };
  setOpt(result, { companyId: optTxt(p, "PartyLegalEntity.CompanyID") });
  return result;
}

function extractTaxRepParty(doc: unknown): ParsedTaxRepParty | undefined {
  const p = get(doc, "TaxRepresentativeParty");
  if (!p) return undefined;
  const name = txt(get(p, "PartyName.Name"));
  if (!name) return undefined;
  const result: ParsedTaxRepParty = { name };
  setOpt(result, { vatNumber: optTxt(p, "PartyTaxScheme.CompanyID"), address: extractAddress(p) });
  return result;
}

function extractAttachments(doc: unknown): ParsedUBLDocument["attachments"] {
  const refs = asArray(get(doc, "AdditionalDocumentReference"));
  if (!refs) return [];
  const attachments: ParsedUBLDocument["attachments"] = [];
  for (const ref of refs) {
    const embedded = get(ref, "Attachment.EmbeddedDocumentBinaryObject");
    if (!embedded) continue;
    const base64Content = txt(embedded);
    if (!base64Content) continue;
    attachments.push({
      id: txt(get(ref, "ID")) || "unknown",
      description: optTxt(ref, "DocumentDescription"),
      mimeCode: txt(get(embedded, "@_mimeCode")) || "application/octet-stream",
      filename: txt(get(embedded, "@_filename")) || "attachment",
      base64Content,
    });
  }
  return attachments;
}

// ─── Main Parser ────────────────────────────────────────────────────────────

export function parseUbl(xmlContent: string): ParsedUBLDocument {
  const parsed = xmlParser.parse(xmlContent);

  let document: unknown;
  let documentType: "invoice" | "credit_note";
  let instanceIdentifier: string | undefined;

  if (parsed.StandardBusinessDocument) {
    const sbd = parsed.StandardBusinessDocument;
    const sbdh = sbd.StandardBusinessDocumentHeader;
    if (sbdh) instanceIdentifier = optTxt(sbdh, "DocumentIdentification.InstanceIdentifier");
    if (sbd.Invoice) { document = sbd.Invoice; documentType = "invoice"; }
    else if (sbd.CreditNote) { document = sbd.CreditNote; documentType = "credit_note"; }
    else throw new Error("Unsupported document type in SBD");
  } else if (parsed.Invoice) { document = parsed.Invoice; documentType = "invoice"; }
  else if (parsed.CreditNote) { document = parsed.CreditNote; documentType = "credit_note"; }
  else throw new Error("Unsupported document format");

  const documentNumber = txt(get(document, "ID"));
  const issueDate = txt(get(document, "IssueDate"));
  if (!documentNumber) throw new Error("Missing required field: document ID");
  if (!issueDate) throw new Error("Missing required field: issue date");

  const typeCodeKey = documentType === "invoice" ? "InvoiceTypeCode" : "CreditNoteTypeCode";
  const monetaryTotal = extractMonetaryTotal(document);
  const { primary: taxTotal, currencyTotal: taxCurrencyTotal } = extractTaxTotals(document);

  const result: ParsedUBLDocument = {
    customizationId: txt(get(document, "CustomizationID")),
    profileId: txt(get(document, "ProfileID")),
    documentNumber,
    documentType,
    typeCode: txt(get(document, typeCodeKey)),
    issueDate,
    currency: txt(get(document, "DocumentCurrencyCode")) || "EUR",
    seller: extractParty(get(document, "AccountingSupplierParty.Party")),
    buyer: extractParty(get(document, "AccountingCustomerParty.Party")),
    monetaryTotal,
    taxTotal,
    lines: extractLines(document, documentType),
    attachments: extractAttachments(document),
  };

  setOpt(result, {
    dueDate: optTxt(document, "DueDate"),
    note: optTxt(document, "Note"),
    taxPointDate: optTxt(document, "TaxPointDate"),
    taxCurrencyCode: optTxt(document, "TaxCurrencyCode"),
    accountingCost: optTxt(document, "AccountingCost"),
    buyerReference: optTxt(document, "BuyerReference"),
    invoicePeriod: extractPeriod(document),
    orderReference: optTxt(document, "OrderReference.ID"),
    salesOrderId: optTxt(document, "OrderReference.SalesOrderID"),
    contractReference: optTxt(document, "ContractDocumentReference.ID"),
    projectReference: optTxt(document, "ProjectReference.ID"),
    despatchDocumentReference: optTxt(document, "DespatchDocumentReference.ID"),
    receiptDocumentReference: optTxt(document, "ReceiptDocumentReference.ID"),
    originatorDocumentReference: optTxt(document, "OriginatorDocumentReference.ID"),
    billingReferences: extractBillingReferences(document),
    payeeParty: extractPayeeParty(document),
    taxRepresentativeParty: extractTaxRepParty(document),
    delivery: extractDelivery(document),
    paymentMeans: extractPaymentMeans(document),
    paymentTerms: optTxt(document, "PaymentTerms.Note"),
    allowanceCharges: asArray(get(document, "AllowanceCharge"))?.map(extractAllowanceCharge),
    taxCurrencyTotal,
    instanceIdentifier,
  });

  return result;
}
