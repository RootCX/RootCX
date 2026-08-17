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

/**
 * EN16931 document level allowance (BG-20) or charge (BG-21).
 *
 * The amount is always POSITIVE — the direction is carried by
 * `cbc:ChargeIndicator` (BIS Billing 3.0 §11.5), never by the sign. This is the
 * only compliant way to express a deduction such as a deposit already invoiced
 * or a global discount: a negative `cbc:PriceAmount` on a line is rejected by
 * EN16931 BR-27 ("The Item net price (BT-146) shall NOT be negative").
 */
export interface DocumentAllowanceCharge {
  /** BT-92 (allowance) / BT-99 (charge). Positive; required by BR-31 / BR-36. */
  amount: number;
  /** BT-96 / BT-103 VAT rate. Must be > 0 for category "S" (BR-S-06). */
  taxPercent: number;
  /** BT-95 / BT-102 VAT category code. Required by BR-32 / BR-37; defaults like a line. */
  taxCategory?: string;
  /** BT-97 / BT-104 free-text reason. Required unless `reasonCode` is set (BR-33 / BR-38). */
  reason?: string;
  /** BT-98 / BT-105 reason code (UNCL5189 for allowances, UNCL7161 for charges). */
  reasonCode?: string;
  /** BT-93 / BT-100 base amount. Only valid together with `percent` (PEPPOL-EN16931-R041/R042). */
  baseAmount?: number;
  /** BT-94 / BT-101 percentage. Only valid together with `baseAmount`. */
  percent?: number;
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
  taxCurrency?: string;
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
  taxAmountInTaxCurrency?: number;
  taxableAmount: number;
  payableAmount: number;
  /** BG-20 document level allowances (deposits already invoiced, global discounts…). */
  allowances?: DocumentAllowanceCharge[];
  /** BG-21 document level charges. */
  charges?: DocumentAllowanceCharge[];
  /** BT-113 amount already paid, VAT included. Subtracted from the payable amount (BR-CO-16). */
  prepaidAmount?: number;
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
  taxCurrency?: string;
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
  taxAmountInTaxCurrency?: number;
  taxableAmount: number;
  payableAmount: number;
  /** BG-20 document level allowances. */
  allowances?: DocumentAllowanceCharge[];
  /** BG-21 document level charges. */
  charges?: DocumentAllowanceCharge[];
  /** BT-113 amount already paid, VAT included (BR-CO-16). */
  prepaidAmount?: number;
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

/**
 * Peppol identifier scheme (ISO 6523) of a `scheme:identifier` participant id.
 * A bare identifier carries no scheme, so it falls back — emitting the
 * identifier itself as `schemeID` would produce an unknown scheme and the
 * document would be refused.
 */
export function resolveSchemeId(peppolId: string, fallback = "0208"): string {
  const [scheme, identifier] = (peppolId ?? "").split(":");
  return scheme && identifier ? scheme : fallback;
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

/**
 * Categories whose VAT rate must be zero: exempt, zero-rated, reverse charge,
 * intra-community, export, out of scope. EN16931 checks this with an exact
 * equality per category (BR-E-05/BR-Z-05/BR-AE-05/BR-IC-05/BR-G-05/BR-O-05), so
 * a rate of 21 % on an exempt discount is a hard rejection, not a rounding
 * difference.
 */
const ZERO_RATE_CATEGORIES = new Set(["E", "Z", "AE", "K", "G", "O"]);

const round2 = (v: number) => Math.round(v * 100) / 100;

interface TaxSubtotal { category: string; percent: number; taxableAmount: number; taxAmount: number; }

/**
 * Everything `cac:LegalMonetaryTotal` needs, already reconciled with the tax
 * breakdown. Amounts are rounded to two decimals (BIS Billing 3.0 §9).
 */
interface DocumentTotals {
  taxSubtotals: TaxSubtotal[];
  taxTotal: number;
  lineExtensionAmount: number;
  taxExclusiveAmount: number;
  taxInclusiveAmount: number;
  allowanceTotalAmount: number;
  chargeTotalAmount: number;
  prepaidAmount: number;
  payableAmount: number;
}

// Rejects documents the Peppol network would refuse anyway, with the rule that
// explains why — a clear throw here beats an opaque `sending_failed` webhook
// hours later.
function assertDocumentLines(lines: InvoiceLine[]): void {
  if (!lines?.length) {
    throw new Error("A document needs at least one line (EN16931 BR-16).");
  }
  for (const line of lines) {
    if ((line.unitPrice ?? 0) < 0) {
      throw new Error(
        `Line ${line.id} ("${line.description}") has a negative unit price (${line.unitPrice}). ` +
        `EN16931 BR-27 forbids a negative Item net price (BT-146). Use a document level ` +
        `allowance (BG-20) for a deduction, or prepaidAmount (BT-113) for an amount already paid.`,
      );
    }
  }
}

function assertAllowanceChargesAreValid(items: DocumentAllowanceCharge[] | undefined, isCharge: boolean): void {
  if (!items?.length) return;
  const kind = isCharge ? "charge" : "allowance";
  // Rule IDs differ between BG-20 (allowance) and BG-21 (charge).
  // BR-32 / BR-37 (a VAT category is required) cannot fail: resolveTaxCategory
  // always yields one.
  const [amountRule, reasonRule] = isCharge ? ["BR-36", "BR-38"] : ["BR-31", "BR-33"];

  for (const item of items) {
    const amount = Number(item.amount);
    if (!Number.isFinite(amount) || amount <= 0) {
      throw new Error(
        `Document level ${kind} needs a positive amount (EN16931 ${amountRule}); got ${item.amount}. ` +
        `The direction is carried by cbc:ChargeIndicator, never by the sign.`,
      );
    }
    if (!item.reason?.trim() && !item.reasonCode?.trim()) {
      throw new Error(`Document level ${kind} of ${amount} needs a reason or a reason code (EN16931 ${reasonRule}).`);
    }
    const percent = Number(item.taxPercent ?? 0);
    if (!Number.isFinite(percent)) {
      throw new Error(`Document level ${kind} of ${amount} has an invalid VAT rate (${item.taxPercent}).`);
    }
    const category = resolveTaxCategory(item.taxCategory, percent);
    if (category === "S" && !(percent > 0)) {
      throw new Error(
        `Document level ${kind} of ${amount} uses VAT category "S" and needs a rate above zero (EN16931 BR-S-06).`,
      );
    }
    if (ZERO_RATE_CATEGORIES.has(category) && percent !== 0) {
      throw new Error(
        `Document level ${kind} of ${amount} uses VAT category "${category}", whose rate must be zero; got ${percent}%.`,
      );
    }
    const hasBase = item.baseAmount != null;
    const hasPercent = item.percent != null;
    if (hasBase !== hasPercent) {
      throw new Error(
        `Document level ${kind} of ${amount} must carry both a base amount and a percentage, or neither ` +
        `(PEPPOL-EN16931-R041/R042).`,
      );
    }
    if (hasBase && Math.abs(round2((item.baseAmount as number) * (item.percent as number) / 100) - amount) > 0.02) {
      throw new Error(
        `Document level ${kind} amount ${amount} must equal base amount × percentage ÷ 100 ` +
        `(PEPPOL-EN16931-R040).`,
      );
    }
  }
}

/**
 * Absorbs the difference between a caller-provided tax total and the one the
 * breakdown adds up to, so BR-CO-14 (TaxTotal = Σ subtotals) still holds.
 *
 * Where the cent lands matters: a zero-rated category is checked with an exact
 * equality (BR-E-09/BR-Z-09/BR-AE-09 — "VAT category tax amount shall equal 0"),
 * so a single cent there is a rejection. Only a taxed category has the ±1
 * tolerance of BR-S-09, hence the filter. Beyond that tolerance no placement is
 * valid and the caller's figure is simply wrong.
 */
function pushResidualOntoSubtotal(subtotals: TaxSubtotal[], residual: number): void {
  if (Math.abs(residual) >= 1) {
    throw new Error(
      `The tax total provided differs from the VAT breakdown by ${residual.toFixed(2)}. ` +
      `EN16931 BR-S-09 only tolerates under one currency unit per VAT category, so no ` +
      `document can carry that difference — recompute the VAT per rate.`,
    );
  }
  const taxed = subtotals.filter((g) => g.percent > 0);
  if (taxed.length === 0) {
    throw new Error(
      `The tax total provided differs from the VAT breakdown by ${residual.toFixed(2)}, but every ` +
      `VAT category on this document is zero-rated and must carry exactly 0 VAT (EN16931 BR-E-09).`,
    );
  }
  const largest = taxed.reduce((max, g) => (g.taxableAmount > max.taxableAmount ? g : max), taxed[0]);
  largest.taxAmount = round2(largest.taxAmount + residual);
}

/**
 * EN16931 BR-CO-10/11/12/13/14/15/16 and BR-S-08.
 *
 * Line net amounts feed `LineExtensionAmount`; document level allowances and
 * charges then move the taxable base — both globally (`TaxExclusiveAmount`) and
 * per VAT category (`TaxSubtotal/TaxableAmount`), which is what BR-S-08 checks.
 *
 * When the caller provides an authoritative tax total (e.g. a workflow using the
 * subtraction method) it takes precedence, and the residual cent is pushed onto
 * the largest taxed subtotal so the breakdown still adds up. That hint is only
 * honoured on a plain document: as soon as an allowance, a charge or a prepaid
 * amount is in play the breakdown is fully determined by BR-S-08 and
 * BR-CO-11/12/13, and a hint computed line by line would only contradict it.
 */
function computeDocumentTotals(params: {
  lines: InvoiceLine[];
  taxableAmount: number;
  authoritativeTaxTotal?: number;
  authoritativePayable?: number;
  allowances?: DocumentAllowanceCharge[];
  charges?: DocumentAllowanceCharge[];
  prepaidAmount?: number;
}): DocumentTotals {
  const { lines, taxableAmount, authoritativeTaxTotal, authoritativePayable } = params;
  const allowances = params.allowances ?? [];
  const charges = params.charges ?? [];
  const prepaidAmount = round2(Number(params.prepaidAmount) || 0);

  const taxGroups = new Map<string, { category: string; percent: number; taxableAmount: number }>();
  const addToTaxGroup = (category: string, percent: number, amount: number) => {
    const key = `${category}:${percent}`;
    const group = taxGroups.get(key) ?? { category, percent, taxableAmount: 0 };
    group.taxableAmount += amount;
    taxGroups.set(key, group);
  };

  for (const line of lines) {
    addToTaxGroup(resolveTaxCategory(line.taxCategory, line.taxPercent), line.taxPercent ?? 21, line.lineAmount);
  }

  // BR-S-08: taxable amount per rate = Σ line net amounts + Σ charges − Σ allowances
  // of that same category and rate. A category that only appears in an
  // allowance/charge still gets its own subtotal.
  const shiftTaxableBase = (items: DocumentAllowanceCharge[], sign: 1 | -1) => {
    for (const item of items) {
      addToTaxGroup(resolveTaxCategory(item.taxCategory, item.taxPercent), item.taxPercent ?? 0, sign * round2(item.amount));
    }
  };
  shiftTaxableBase(allowances, -1);
  shiftTaxableBase(charges, 1);

  const taxSubtotals: TaxSubtotal[] = Array.from(taxGroups.values()).map(g => ({
    ...g,
    taxableAmount: round2(g.taxableAmount),
    taxAmount: Math.round(g.taxableAmount * g.percent) / 100,
  }));

  // BR-CO-11/12: the allowance/charge total is the sum of the amounts as they
  // are written in the document, so each one is rounded before it is summed —
  // summing first would drift a cent away from the elements on sub-cent amounts.
  const allowanceTotalAmount = round2(allowances.reduce((s, a) => s + round2(Number(a.amount)), 0));
  const chargeTotalAmount = round2(charges.reduce((s, c) => s + round2(Number(c.amount)), 0));
  const hasAdjustments = allowanceTotalAmount !== 0 || chargeTotalAmount !== 0 || prepaidAmount !== 0;

  const computedTotal = taxSubtotals.reduce((s, g) => s + g.taxAmount, 0);
  const useTaxTotal = !hasAdjustments && authoritativeTaxTotal && authoritativeTaxTotal !== computedTotal;
  const taxTotal = useTaxTotal ? authoritativeTaxTotal : computedTotal;

  if (useTaxTotal && Math.abs(authoritativeTaxTotal - computedTotal) > 0.001) {
    pushResidualOntoSubtotal(taxSubtotals, round2(authoritativeTaxTotal - computedTotal));
  }

  const lineExtensionAmount = round2(taxableAmount);
  const taxExclusiveAmount = round2(lineExtensionAmount - allowanceTotalAmount + chargeTotalAmount);
  const computedInclusive = round2(taxExclusiveAmount + taxTotal);

  // Legacy escape hatch: callers that only know the gross total (and no
  // allowance/charge/prepaid) may pin TaxInclusiveAmount to it. Once any of the
  // new elements is in play the totals are fully determined by BR-CO-13/15/16,
  // so the hint must be ignored or the document contradicts itself.
  const usePayable = !hasAdjustments
    && authoritativePayable && authoritativePayable > taxableAmount && authoritativePayable !== computedInclusive;
  const taxInclusiveAmount = usePayable ? authoritativePayable : computedInclusive;

  return {
    taxSubtotals,
    taxTotal,
    lineExtensionAmount,
    taxExclusiveAmount,
    taxInclusiveAmount,
    allowanceTotalAmount,
    chargeTotalAmount,
    prepaidAmount,
    payableAmount: round2(taxInclusiveAmount - prepaidAmount),
  };
}

const noteEl = (note?: string) =>
  note ? `\n    <cbc:Note>${escapeXml(note)}</cbc:Note>` : "";
const taxCurrencyCodeEl = (documentCurrency: string, taxCurrency?: string) =>
  taxCurrency && taxCurrency !== documentCurrency ? `\n    <cbc:TaxCurrencyCode>${taxCurrency}</cbc:TaxCurrencyCode>` : "";
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
  const schemeId = resolveSchemeId(supplier.peppolId);
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
            <cbc:EndpointID schemeID="${resolveSchemeId(customer.peppolId, schemeId)}">${customerId}</cbc:EndpointID>
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

function taxCurrencyTotalEl(documentCurrency: string, taxCurrency: string | undefined, taxAmount: number | undefined): string {
  if (!taxCurrency || taxCurrency === documentCurrency || taxAmount == null) return "";
  return `
    <cac:TaxTotal>
        <cbc:TaxAmount currencyID="${taxCurrency}">${taxAmount.toFixed(2)}</cbc:TaxAmount>
    </cac:TaxTotal>`;
}

function assertTaxCurrencyComplete(documentCurrency: string, taxCurrency: string | undefined, taxAmount: number | undefined): void {
  if (taxCurrency && taxCurrency !== documentCurrency && taxAmount == null) {
    throw new Error("taxAmountInTaxCurrency is required when taxCurrency differs from currency");
  }
}

// Element order is fixed by UBL's MonetaryTotalType: LineExtensionAmount,
// TaxExclusiveAmount, TaxInclusiveAmount, AllowanceTotalAmount,
// ChargeTotalAmount, PrepaidAmount, PayableRoundingAmount, PayableAmount.
function legalMonetaryTotalEl(totals: DocumentTotals, currency: string): string {
  const amount = (tag: string, value: number) =>
    `\n        <cbc:${tag} currencyID="${currency}">${value.toFixed(2)}</cbc:${tag}>`;
  return `<cac:LegalMonetaryTotal>${amount("LineExtensionAmount", totals.lineExtensionAmount)}${amount("TaxExclusiveAmount", totals.taxExclusiveAmount)}${amount("TaxInclusiveAmount", totals.taxInclusiveAmount)}${
    totals.allowanceTotalAmount !== 0 ? amount("AllowanceTotalAmount", totals.allowanceTotalAmount) : ""}${
    totals.chargeTotalAmount !== 0 ? amount("ChargeTotalAmount", totals.chargeTotalAmount) : ""}${
    totals.prepaidAmount !== 0 ? amount("PrepaidAmount", totals.prepaidAmount) : ""}${amount("PayableAmount", totals.payableAmount)}
    </cac:LegalMonetaryTotal>`;
}

// Document level allowances (BG-20) and charges (BG-21). Sits between
// cac:PaymentTerms and cac:TaxTotal, as required by the UBL Invoice/CreditNote
// schema; element order inside follows UBL's AllowanceChargeType.
function allowanceChargeEls(
  allowances: DocumentAllowanceCharge[] | undefined,
  charges: DocumentAllowanceCharge[] | undefined,
  currency: string,
): string {
  const one = (item: DocumentAllowanceCharge, isCharge: boolean) => `
    <cac:AllowanceCharge>
        <cbc:ChargeIndicator>${isCharge}</cbc:ChargeIndicator>${item.reasonCode?.trim() ? `
        <cbc:AllowanceChargeReasonCode>${escapeXml(item.reasonCode.trim())}</cbc:AllowanceChargeReasonCode>` : ""}${item.reason?.trim() ? `
        <cbc:AllowanceChargeReason>${escapeXml(item.reason.trim())}</cbc:AllowanceChargeReason>` : ""}${item.percent != null ? `
        <cbc:MultiplierFactorNumeric>${item.percent}</cbc:MultiplierFactorNumeric>` : ""}
        <cbc:Amount currencyID="${currency}">${round2(item.amount).toFixed(2)}</cbc:Amount>${item.baseAmount != null ? `
        <cbc:BaseAmount currencyID="${currency}">${round2(item.baseAmount).toFixed(2)}</cbc:BaseAmount>` : ""}
        <cac:TaxCategory>
            <cbc:ID>${resolveTaxCategory(item.taxCategory, item.taxPercent)}</cbc:ID>${
    // BR-O-05: an out-of-scope allowance/charge carries no rate at all.
    resolveTaxCategory(item.taxCategory, item.taxPercent) === "O" ? "" : `
            <cbc:Percent>${item.taxPercent ?? 0}</cbc:Percent>`}
            <cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme>
        </cac:TaxCategory>
    </cac:AllowanceCharge>`;

  return [
    ...(allowances ?? []).map((a) => one(a, false)),
    ...(charges ?? []).map((c) => one(c, true)),
  ].join("");
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
    invoiceNumber, issueDate, dueDate, currency = "EUR", taxCurrency,
    buyerReference = invoiceNumber, supplier, customer, lines, taxableAmount, note,
  } = params;

  const schemeId = resolveSchemeId(supplier.peppolId);
  assertDocumentLines(lines);
  assertAllowanceChargesAreValid(params.allowances, false);
  assertAllowanceChargesAreValid(params.charges, true);
  const totals = computeDocumentTotals({
    lines, taxableAmount,
    authoritativeTaxTotal: params.taxTotal,
    authoritativePayable: params.payableAmount,
    allowances: params.allowances,
    charges: params.charges,
    prepaidAmount: params.prepaidAmount,
  });
  assertTaxCurrencyComplete(currency, taxCurrency, params.taxAmountInTaxCurrency);

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
    <cbc:DocumentCurrencyCode>${currency}</cbc:DocumentCurrencyCode>${taxCurrencyCodeEl(currency, taxCurrency)}
    <cbc:BuyerReference>${escapeXml(buyerReference)}</cbc:BuyerReference>${orderRefEl(params.orderReference)}${originatorRefEl(params.originatorReference)}${contractRefEl(params.contractReference)}${docRefEls(params.documentReferences)}${projectRefEl(params.projectReference)}
    ${supplierPartyEl(supplier)}
    ${customerPartyEl(customer, schemeId)}${paymentMeansEl(params.paymentInfo, invoiceNumber)}${allowanceChargeEls(params.allowances, params.charges, currency)}
    ${taxTotalEl(totals.taxSubtotals, totals.taxTotal, currency)}${taxCurrencyTotalEl(currency, taxCurrency, params.taxAmountInTaxCurrency)}
    ${legalMonetaryTotalEl(totals, currency)}${documentLinesEl(lines, currency, "InvoiceLine", "InvoicedQuantity")}
</Invoice>`;
}

// ─── Credit Note ───────────────────────────────────────────────────────────────

export function generateCreditNoteXml(params: CreditNoteParams): string {
  const {
    creditNoteNumber, issueDate, currency = "EUR", taxCurrency,
    correctedInvoiceNumber, correctedInvoiceDate,
    buyerReference = creditNoteNumber, supplier, customer, lines, taxableAmount, note,
    paymentTermsNote = "Credit note relating to the referenced invoice.",
  } = params;

  if (!correctedInvoiceNumber) throw new Error("correctedInvoiceNumber is required for a credit note");

  const schemeId = resolveSchemeId(supplier.peppolId);
  assertDocumentLines(lines);
  assertAllowanceChargesAreValid(params.allowances, false);
  assertAllowanceChargesAreValid(params.charges, true);
  const totals = computeDocumentTotals({
    lines, taxableAmount,
    authoritativeTaxTotal: params.taxTotal,
    authoritativePayable: params.payableAmount,
    allowances: params.allowances,
    charges: params.charges,
    prepaidAmount: params.prepaidAmount,
  });
  assertTaxCurrencyComplete(currency, taxCurrency, params.taxAmountInTaxCurrency);

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
    <cbc:DocumentCurrencyCode>${currency}</cbc:DocumentCurrencyCode>${taxCurrencyCodeEl(currency, taxCurrency)}
    <cbc:BuyerReference>${escapeXml(buyerReference)}</cbc:BuyerReference>${orderRefEl(params.orderReference)}${billingRefEl}${contractRefEl(params.contractReference)}${docRefEls(params.documentReferences)}${originatorRefEl(params.originatorReference)}
    ${supplierPartyEl(supplier)}
    ${customerPartyEl(customer, schemeId)}${paymentMeansEl(params.paymentInfo, creditNoteNumber)}
    <cac:PaymentTerms>
        <cbc:Note>${escapeXml(paymentTermsNote)}</cbc:Note>
    </cac:PaymentTerms>${allowanceChargeEls(params.allowances, params.charges, currency)}
    ${taxTotalEl(totals.taxSubtotals, totals.taxTotal, currency)}${taxCurrencyTotalEl(currency, taxCurrency, params.taxAmountInTaxCurrency)}
    ${legalMonetaryTotalEl(totals, currency)}${documentLinesEl(lines, currency, "CreditNoteLine", "CreditedQuantity")}
</CreditNote>`;
}
