import { describe, it, expect } from "vitest";
import { generateInvoiceXml, generateCreditNoteXml } from "./generateUbl";
import { parseUbl } from "./parseUbl";

const baseSupplier = {
  peppolId: "0208:1034898146",
  name: "Test Supplier",
  vatNumber: "BE1034898146",
  street: "Rue Test 1",
  city: "Bruxelles",
  postalCode: "1000",
  countryCode: "BE",
};

const baseCustomer = {
  peppolId: "0208:0794263219",
  name: "Test Customer",
  vatNumber: "BE0794263219",
  street: "Avenue Test 2",
  city: "Liege",
  postalCode: "4000",
  countryCode: "BE",
};

function extractAllTaxSubtotalAmounts(xml: string): number[] {
  const amounts: number[] = [];
  // A subtotal amount can be negative (an allowance at a rate no line uses).
  const re = /<cac:TaxSubtotal>[\s\S]*?<cbc:TaxAmount[^>]*>(-?[\d.]+)<\/cbc:TaxAmount>[\s\S]*?<\/cac:TaxSubtotal>/g;
  let m;
  while ((m = re.exec(xml)) !== null) amounts.push(parseFloat(m[1]));
  return amounts;
}

function extractTaxTotals(xml: string): { currency: string; amount: string; hasSubtotals: boolean }[] {
  const totals: { currency: string; amount: string; hasSubtotals: boolean }[] = [];
  const re = /<cac:TaxTotal>([\s\S]*?)<\/cac:TaxTotal>/g;
  let m;
  while ((m = re.exec(xml)) !== null) {
    const body = m[1];
    const amount = body.match(/<cbc:TaxAmount currencyID="([^"]+)">([\d.]+)<\/cbc:TaxAmount>/);
    if (amount) totals.push({ currency: amount[1], amount: amount[2], hasSubtotals: body.includes("<cac:TaxSubtotal>") });
  }
  return totals;
}

describe("generateInvoiceXml BR-CO-14 compliance", () => {
  const cases = [
    {
      name: "real failure: 6% + 21% mixed rates causing 1-cent rounding drift",
      lines: [
        { id: "1", description: "Nourriture", quantity: 1, unitPrice: 623.25, taxPercent: 6, lineAmount: 623.25 },
        { id: "2", description: "Personnel et Boisson", quantity: 1, unitPrice: 516.60, taxPercent: 21, lineAmount: 516.60 },
      ],
    },
    {
      name: "edge case: multiple lines same rate, fractional cents",
      lines: [
        { id: "1", description: "Item A", quantity: 1, unitPrice: 33.33, taxPercent: 21, lineAmount: 33.33 },
        { id: "2", description: "Item B", quantity: 1, unitPrice: 66.67, taxPercent: 21, lineAmount: 66.67 },
        { id: "3", description: "Item C", quantity: 1, unitPrice: 10.01, taxPercent: 6, lineAmount: 10.01 },
      ],
    },
    {
      name: "edge case: amount that rounds to .5 cent (banker's rounding boundary)",
      lines: [
        { id: "1", description: "Service", quantity: 1, unitPrice: 123.45, taxPercent: 21, lineAmount: 123.45 },
        { id: "2", description: "Transport", quantity: 1, unitPrice: 78.55, taxPercent: 6, lineAmount: 78.55 },
      ],
    },
    {
      name: "many lines, same tax rate, cumulative rounding",
      lines: Array.from({ length: 10 }, (_, i) => ({
        id: String(i + 1),
        description: `Item ${i + 1}`,
        quantity: 1,
        unitPrice: 9.99,
        taxPercent: 21,
        lineAmount: 9.99,
      })),
    },
  ];

  for (const { name, lines } of cases) {
    it(`BR-CO-14 + BR-CO-15: TaxTotal == sum(TaxSubtotal), TaxInclusive == TaxExclusive + TaxTotal - ${name}`, () => {
      const taxableAmount = lines.reduce((s, l) => s + l.lineAmount, 0);
      const xml = generateInvoiceXml({
        invoiceNumber: "TEST-001",
        issueDate: "2026-05-18",
        dueDate: "2026-06-17",
        supplier: baseSupplier,
        customer: baseCustomer,
        lines,
        taxTotal: 0,
        taxableAmount,
        payableAmount: taxableAmount,
      });

      const taxTotalMatch = xml.match(/<cac:TaxTotal>\s*<cbc:TaxAmount[^>]*>([\d.]+)<\/cbc:TaxAmount>/);
      expect(taxTotalMatch, "TaxTotal element must exist").toBeTruthy();
      const totalVat = parseFloat(taxTotalMatch![1]);

      const subtotalAmounts = extractAllTaxSubtotalAmounts(xml);
      expect(subtotalAmounts.length).toBeGreaterThan(0);

      const sumOfSubtotals = subtotalAmounts.reduce((s, a) => s + a, 0);
      const sumRounded = Math.round(sumOfSubtotals * 100) / 100;

      expect(totalVat).toBe(sumRounded);

      // BR-CO-15: TaxInclusiveAmount == TaxExclusiveAmount + TaxTotal
      const taxInclusiveMatch = xml.match(/<cbc:TaxInclusiveAmount[^>]*>([\d.]+)<\/cbc:TaxInclusiveAmount>/);
      const taxExclusiveMatch = xml.match(/<cbc:TaxExclusiveAmount[^>]*>([\d.]+)<\/cbc:TaxExclusiveAmount>/);
      const inclusive = parseFloat(taxInclusiveMatch![1]);
      const exclusive = parseFloat(taxExclusiveMatch![1]);
      expect(inclusive).toBe(Math.round((exclusive + totalVat) * 100) / 100);
    });
  }
});

describe("generateInvoiceXml accounting tax currency", () => {
  it("emits invoice currency totals in USD and a separate VAT accounting total in EUR", () => {
    const xml = generateInvoiceXml({
      invoiceNumber: "INV-USD-BE-001",
      issueDate: "2026-07-02",
      dueDate: "2026-07-02",
      currency: "USD",
      taxCurrency: "EUR",
      supplier: baseSupplier,
      customer: baseCustomer,
      lines: [{ id: "1", description: "RootCX Pro", quantity: 1, unitPrice: 150, taxPercent: 21, lineAmount: 150 }],
      taxTotal: 31.5,
      taxAmountInTaxCurrency: 29.3,
      taxableAmount: 150,
      payableAmount: 181.5,
    });

    expect(xml).toContain("<cbc:DocumentCurrencyCode>USD</cbc:DocumentCurrencyCode>");
    expect(xml).toContain("<cbc:TaxCurrencyCode>EUR</cbc:TaxCurrencyCode>");

    const totals = extractTaxTotals(xml);
    expect(totals).toEqual([
      { currency: "USD", amount: "31.50", hasSubtotals: true },
      { currency: "EUR", amount: "29.30", hasSubtotals: false },
    ]);
    expect(xml).toContain('<cbc:PayableAmount currencyID="USD">181.50</cbc:PayableAmount>');
  });

  it("rejects a different tax currency when the converted VAT amount is missing", () => {
    expect(() => generateInvoiceXml({
      invoiceNumber: "INV-USD-BE-002",
      issueDate: "2026-07-02",
      dueDate: "2026-07-02",
      currency: "USD",
      taxCurrency: "EUR",
      supplier: baseSupplier,
      customer: baseCustomer,
      lines: [{ id: "1", description: "RootCX Pro", quantity: 1, unitPrice: 150, taxPercent: 21, lineAmount: 150 }],
      taxTotal: 31.5,
      taxableAmount: 150,
      payableAmount: 181.5,
    })).toThrow(/taxAmountInTaxCurrency/);
  });

  it("generates accounting tax currency data that RootCX can parse back", () => {
    const xml = generateInvoiceXml({
      invoiceNumber: "INV-USD-BE-003",
      issueDate: "2026-07-02",
      dueDate: "2026-07-02",
      currency: "USD",
      taxCurrency: "EUR",
      supplier: baseSupplier,
      customer: baseCustomer,
      lines: [{ id: "1", description: "RootCX Pro", quantity: 1, unitPrice: 150, taxPercent: 21, lineAmount: 150 }],
      taxTotal: 31.5,
      taxAmountInTaxCurrency: 29.3,
      taxableAmount: 150,
      payableAmount: 181.5,
    });

    const parsed = parseUbl(xml);
    expect(parsed.currency).toBe("USD");
    expect(parsed.taxCurrencyCode).toBe("EUR");
    expect(parsed.taxTotal.taxAmount).toBe(31.5);
    expect(parsed.taxCurrencyTotal).toBe(29.3);
  });
});

describe("generateCreditNoteXml structure (Peppol BIS 3.0 CreditNote)", () => {
  const lines = [
    { id: "1", description: "Nourriture", quantity: 1, unitPrice: 623.25, taxPercent: 6, lineAmount: 623.25 },
    { id: "2", description: "Service", quantity: 2, unitPrice: 100, taxPercent: 21, lineAmount: 200 },
  ];
  const taxableAmount = lines.reduce((s, l) => s + l.lineAmount, 0);

  const build = (over: Record<string, unknown> = {}) =>
    generateCreditNoteXml({
      creditNoteNumber: "CN-20260615-001",
      issueDate: "2026-06-15",
      correctedInvoiceNumber: "INV-20260601-007",
      correctedInvoiceDate: "2026-06-01",
      supplier: baseSupplier,
      customer: baseCustomer,
      lines,
      taxTotal: 0,
      taxableAmount,
      payableAmount: taxableAmount,
      note: "TVA à reverser à l'État dans la mesure où elle a été initialement déduite",
      ...over,
    });

  it("uses the CreditNote root, namespace and type code 381 (not Invoice/380)", () => {
    const xml = build();
    expect(xml).toContain("<CreditNote xmlns=\"urn:oasis:names:specification:ubl:schema:xsd:CreditNote-2\"");
    expect(xml).toContain("<cbc:CreditNoteTypeCode>381</cbc:CreditNoteTypeCode>");
    expect(xml).not.toContain("<cbc:InvoiceTypeCode>");
    expect(xml).not.toContain("<Invoice ");
    expect(xml.trimEnd().endsWith("</CreditNote>")).toBe(true);
  });

  it("references the corrected invoice number + date via BillingReference", () => {
    const xml = build();
    expect(xml).toMatch(
      /<cac:BillingReference>\s*<cac:InvoiceDocumentReference>\s*<cbc:ID>INV-20260601-007<\/cbc:ID>\s*<cbc:IssueDate>2026-06-01<\/cbc:IssueDate>/,
    );
  });

  it("uses CreditNoteLine / CreditedQuantity, never InvoiceLine / InvoicedQuantity", () => {
    const xml = build();
    expect(xml).toContain("<cac:CreditNoteLine>");
    expect(xml).toContain("<cbc:CreditedQuantity");
    expect(xml).not.toContain("<cac:InvoiceLine>");
    expect(xml).not.toContain("<cbc:InvoicedQuantity");
  });

  it("emits PaymentTerms (BR-CO-25) and carries the user note verbatim, no hardcoded jurisdiction text", () => {
    const xml = build();
    expect(xml).toContain("<cac:PaymentTerms>");
    expect(xml).toContain("TVA à reverser à l&apos;État dans la mesure où elle a été initialement déduite");
    // No BillingReference text should leak unless provided; no hardcoded BE strings when note is empty
    const plain = build({ note: undefined });
    expect(plain).not.toMatch(/TVA à reverser|BTW terug te storten/);
  });

  it("has no root DueDate element (CreditNote has none)", () => {
    expect(build()).not.toContain("<cbc:DueDate>");
  });

  it("throws when the corrected invoice number is missing", () => {
    expect(() => build({ correctedInvoiceNumber: "" })).toThrow(/correctedInvoiceNumber/);
  });

  it("keeps element ordering: BillingReference after BuyerReference, before AccountingSupplierParty", () => {
    const xml = build();
    const buyer = xml.indexOf("<cbc:BuyerReference>");
    const billing = xml.indexOf("<cac:BillingReference>");
    const supplier = xml.indexOf("<cac:AccountingSupplierParty>");
    const terms = xml.indexOf("<cac:PaymentTerms>");
    const tax = xml.indexOf("<cac:TaxTotal>");
    expect(buyer).toBeGreaterThan(-1);
    expect(buyer).toBeLessThan(billing);
    expect(billing).toBeLessThan(supplier);
    expect(terms).toBeLessThan(tax);
  });

  it("BR-CO-14 / BR-CO-15: TaxTotal == sum(TaxSubtotal), Inclusive == Exclusive + Tax", () => {
    const xml = build();
    const totalVat = parseFloat(xml.match(/<cac:TaxTotal>\s*<cbc:TaxAmount[^>]*>([\d.]+)</)![1]);
    const sumSub = Math.round(extractAllTaxSubtotalAmounts(xml).reduce((s, a) => s + a, 0) * 100) / 100;
    expect(totalVat).toBe(sumSub);
    const inclusive = parseFloat(xml.match(/<cbc:TaxInclusiveAmount[^>]*>([\d.]+)</)![1]);
    const exclusive = parseFloat(xml.match(/<cbc:TaxExclusiveAmount[^>]*>([\d.]+)</)![1]);
    expect(inclusive).toBe(Math.round((exclusive + totalVat) * 100) / 100);
  });

  it("omits IssueDate inside BillingReference when correctedInvoiceDate is absent", () => {
    const xml = build({ correctedInvoiceDate: undefined });
    expect(xml).toMatch(/<cac:InvoiceDocumentReference>\s*<cbc:ID>INV-20260601-007<\/cbc:ID>\s*<\/cac:InvoiceDocumentReference>/);
    expect(xml).not.toMatch(/<cac:InvoiceDocumentReference>[\s\S]*<cbc:IssueDate><\/cbc:IssueDate>/);
  });

  it("emits optional references in correct CreditNote order (Order → Billing → Contract → Additional → Originator)", () => {
    const xml = build({
      orderReference: "PO-123",
      contractReference: "CTR-456",
      originatorReference: "ORIG-789",
      documentReferences: [{ id: "ATT-1", description: "specs" }],
    });
    const order = xml.indexOf("PO-123");
    const billing = xml.indexOf("<cac:BillingReference>");
    const contract = xml.indexOf("CTR-456");
    const additional = xml.indexOf("ATT-1");
    const originator = xml.indexOf("ORIG-789");
    const supplier = xml.indexOf("<cac:AccountingSupplierParty>");
    // All present
    for (const [name, pos] of [["order", order], ["billing", billing], ["contract", contract], ["additional", additional], ["originator", originator]] as const) {
      expect(pos, `${name} must be present`).toBeGreaterThan(-1);
    }
    // Correct order per Peppol CreditNote tree
    expect(order).toBeLessThan(billing);
    expect(billing).toBeLessThan(contract);
    expect(contract).toBeLessThan(additional);
    expect(additional).toBeLessThan(originator);
    expect(originator).toBeLessThan(supplier);
  });

  it("emits PaymentMeans with IBAN/BIC when paymentInfo is provided", () => {
    const xml = build({ paymentInfo: { iban: "BE68539007547034", bic: "GKCCBEBB" } });
    expect(xml).toContain("<cac:PaymentMeans>");
    expect(xml).toContain("BE68539007547034");
    expect(xml).toContain("GKCCBEBB");
    // PaymentMeans must be before PaymentTerms
    expect(xml.indexOf("<cac:PaymentMeans>")).toBeLessThan(xml.indexOf("<cac:PaymentTerms>"));
  });

  it("uses the specified currency throughout, not hardcoded EUR", () => {
    const xml = build({ currency: "USD" });
    expect(xml).toContain("<cbc:DocumentCurrencyCode>USD</cbc:DocumentCurrencyCode>");
    expect(xml).toContain('currencyID="USD"');
    expect(xml).not.toContain('currencyID="EUR"');
  });

  it("emits a separate EUR VAT accounting total for USD credit notes", () => {
    const xml = build({ currency: "USD", taxCurrency: "EUR", taxAmountInTaxCurrency: 73.84 });

    expect(xml).toContain("<cbc:DocumentCurrencyCode>USD</cbc:DocumentCurrencyCode>");
    expect(xml).toContain("<cbc:TaxCurrencyCode>EUR</cbc:TaxCurrencyCode>");
    expect(extractTaxTotals(xml)).toEqual([
      { currency: "USD", amount: "79.40", hasSubtotals: true },
      { currency: "EUR", amount: "73.84", hasSubtotals: false },
    ]);
  });

  it("uses custom paymentTermsNote when provided, default when not", () => {
    const custom = build({ paymentTermsNote: "Net 30 days" });
    expect(custom).toContain("<cbc:Note>Net 30 days</cbc:Note>");
    // Default (no paymentTermsNote key) — must still emit PaymentTerms for BR-CO-25
    const defaults = build();
    expect(defaults).toContain("Credit note relating to the referenced invoice.");
  });

  it("escapes XML-special characters in all user-provided fields", () => {
    const xml = build({
      creditNoteNumber: "CN&<01",
      correctedInvoiceNumber: "INV-'\"&-007",
      note: "A<B & C>D 'single' \"double\"",
      supplier: { ...baseSupplier, name: "Firm & Co <SRL>" },
      customer: { ...baseCustomer, name: "Client \"Best\" <SA>" },
      lines: [{ id: "1", description: "Item <&> \"test\"", quantity: 1, unitPrice: 100, taxPercent: 21, lineAmount: 100 }],
    });
    expect(xml).toContain("CN&amp;&lt;01");
    expect(xml).toContain("INV-&apos;&quot;&amp;-007");
    expect(xml).toContain("A&lt;B &amp; C&gt;D &apos;single&apos; &quot;double&quot;");
    expect(xml).toContain("Firm &amp; Co &lt;SRL&gt;");
    expect(xml).toContain("Item &lt;&amp;&gt; &quot;test&quot;");
  });
});

describe("authoritative totals (subtraction method)", () => {
  it("invoice uses caller-provided taxTotal and payableAmount over line recalculation", () => {
    // 55 USD TTC Belgian refund: net=45.45, tax by subtraction=9.55
    // Multiplication would give 45.45*0.21=9.5445 rounded to 9.54 (WRONG)
    const xml = generateInvoiceXml({
      invoiceNumber: "INV-AUTH-001",
      issueDate: "2026-05-04",
      dueDate: "2026-05-04",
      currency: "USD",
      supplier: baseSupplier,
      customer: baseCustomer,
      lines: [{ id: "1", description: "RootCX Pro", quantity: 3, unitPrice: 20.66, taxPercent: 21, lineAmount: 61.98 },
              { id: "2", description: "AI Credits", quantity: 1, unitPrice: 82.65, taxPercent: 21, lineAmount: 82.65 }],
      taxTotal: 30.37,
      taxableAmount: 144.63,
      payableAmount: 175,
    });

    expect(xml).toContain('<cbc:TaxAmount currencyID="USD">30.37</cbc:TaxAmount>');
    expect(xml).toContain('<cbc:TaxInclusiveAmount currencyID="USD">175.00</cbc:TaxInclusiveAmount>');
    expect(xml).toContain('<cbc:PayableAmount currencyID="USD">175.00</cbc:PayableAmount>');
    const subtotalAmounts = extractAllTaxSubtotalAmounts(xml);
    expect(subtotalAmounts.reduce((s, a) => s + a, 0)).toBe(30.37);
  });

  it("credit note uses caller-provided taxTotal and payableAmount", () => {
    const xml = generateCreditNoteXml({
      creditNoteNumber: "CN-AUTH-001",
      issueDate: "2026-05-14",
      currency: "USD",
      correctedInvoiceNumber: "INV-20260504-001",
      supplier: baseSupplier,
      customer: baseCustomer,
      lines: [{ id: "1", description: "Stripe refund", quantity: 1, unitPrice: 45.45, taxPercent: 21, lineAmount: 45.45 }],
      taxTotal: 9.55,
      taxableAmount: 45.45,
      payableAmount: 55,
    });

    expect(xml).toContain('<cbc:TaxAmount currencyID="USD">9.55</cbc:TaxAmount>');
    expect(xml).toContain('<cbc:TaxInclusiveAmount currencyID="USD">55.00</cbc:TaxInclusiveAmount>');
    expect(xml).toContain('<cbc:PayableAmount currencyID="USD">55.00</cbc:PayableAmount>');
  });

  it("falls back to computed totals when taxTotal/payableAmount are not provided", () => {
    const xml = generateInvoiceXml({
      invoiceNumber: "INV-NOAUTH-001",
      issueDate: "2026-05-04",
      dueDate: "2026-05-04",
      supplier: baseSupplier,
      customer: baseCustomer,
      lines: [{ id: "1", description: "Service", quantity: 1, unitPrice: 100, taxPercent: 21, lineAmount: 100 }],
      taxTotal: 0,
      taxableAmount: 100,
      payableAmount: 100,
    });

    // taxTotal=0 means "not authoritative" — generator should compute 100*21%=21
    expect(xml).toContain('<cbc:TaxAmount currencyID="EUR">21.00</cbc:TaxAmount>');
    expect(xml).toContain('<cbc:TaxInclusiveAmount currencyID="EUR">121.00</cbc:TaxInclusiveAmount>');
  });
});

// ─── Negative prices, deposits and discounts ──────────────────────────────────
//
// EN16931 BR-27 ("The Item net price (BT-146) shall NOT be negative", bound in
// UBL to `cac:Price/cbc:PriceAmount >= 0`) makes a "deduct the deposit" line
// impossible. The compliant constructs are a document level allowance (BG-20)
// and the paid amount (BT-113). These tests pin both.

const invoiceBase = {
  invoiceNumber: "INV-ALLOW-001",
  issueDate: "2026-08-17",
  dueDate: "2026-09-16",
  supplier: baseSupplier,
  customer: baseCustomer,
  taxTotal: 0,
  taxableAmount: 0,
  payableAmount: 0,
};

describe("BR-27 — negative item price is refused before it reaches the network", () => {
  const negativeLines = [
    { id: "1", description: "Prestation", quantity: 1, unitPrice: 5000, taxPercent: 21, lineAmount: 5000 },
    { id: "2", description: "Déduction Accompte (21%)", quantity: 1, unitPrice: -1000, taxPercent: 21, lineAmount: -1000 },
  ];

  it("throws on an invoice, naming the rule and the alternatives", () => {
    expect(() => generateInvoiceXml({ ...invoiceBase, lines: negativeLines, taxableAmount: 4000 }))
      .toThrow(/BR-27/);
    expect(() => generateInvoiceXml({ ...invoiceBase, lines: negativeLines, taxableAmount: 4000 }))
      .toThrow(/BG-20|prepaidAmount/);
  });

  it("throws on a credit note too (BR-27 applies to CreditNoteLine)", () => {
    expect(() => generateCreditNoteXml({
      creditNoteNumber: "CN-NEG-001",
      issueDate: "2026-08-17",
      correctedInvoiceNumber: "INV-20260719-003",
      supplier: baseSupplier,
      customer: baseCustomer,
      lines: negativeLines,
      taxTotal: 0,
      taxableAmount: 4000,
      payableAmount: 0,
    })).toThrow(/BR-27/);
  });

  it("still accepts a zero price and a negative line amount from a negative quantity", () => {
    // BIS Billing 3.0 §5.6.1: "Invoice line 2 with negative quantity and line
    // amount" — only the price has to stay positive.
    const xml = generateInvoiceXml({
      ...invoiceBase,
      lines: [
        { id: "1", description: "Free sample", quantity: 1, unitPrice: 0, taxPercent: 21, lineAmount: 0 },
        { id: "2", description: "Returned goods", quantity: -2, unitPrice: 50, taxPercent: 21, lineAmount: -100 },
      ],
      taxableAmount: -100,
    });
    expect(xml).toContain('<cbc:PriceAmount currencyID="EUR">50.00</cbc:PriceAmount>');
    expect(xml).toContain('<cbc:LineExtensionAmount currencyID="EUR">-100.00</cbc:LineExtensionAmount>');
  });
});

describe("BT-113 prepaid amount", () => {
  // The real failure: INV-20260614-001 deducted an already-invoiced deposit with
  // two negative lines. Expressed as a paid amount it is valid and the payable
  // amount drops accordingly (BR-CO-16).
  const xml = generateInvoiceXml({
    ...invoiceBase,
    lines: [{ id: "1", description: "Prestation traiteur", quantity: 1, unitPrice: 5000, taxPercent: 21, lineAmount: 5000 }],
    taxableAmount: 5000,
    prepaidAmount: 1210,
  });

  it("subtracts the paid amount from the payable amount (BR-CO-16)", () => {
    expect(xml).toContain('<cbc:TaxInclusiveAmount currencyID="EUR">6050.00</cbc:TaxInclusiveAmount>');
    expect(xml).toContain('<cbc:PrepaidAmount currencyID="EUR">1210.00</cbc:PrepaidAmount>');
    expect(xml).toContain('<cbc:PayableAmount currencyID="EUR">4840.00</cbc:PayableAmount>');
  });

  it("leaves the taxable base and the VAT untouched", () => {
    expect(xml).toContain('<cbc:LineExtensionAmount currencyID="EUR">5000.00</cbc:LineExtensionAmount>');
    expect(xml).toContain('<cbc:TaxExclusiveAmount currencyID="EUR">5000.00</cbc:TaxExclusiveAmount>');
    expect(xml).toContain('<cbc:TaxAmount currencyID="EUR">1050.00</cbc:TaxAmount>');
  });

  // Element order inside LegalMonetaryTotal is covered once, with every optional
  // amount present, in "per-rate reconciliation of allowances and charges".

  it("omits PrepaidAmount entirely when there is nothing paid upfront", () => {
    expect(generateInvoiceXml({ ...invoiceBase, lines: [{ id: "1", description: "X", quantity: 1, unitPrice: 100, taxPercent: 21, lineAmount: 100 }], taxableAmount: 100 }))
      .not.toContain("PrepaidAmount");
  });

  it("works on a credit note as well", () => {
    const cn = generateCreditNoteXml({
      creditNoteNumber: "CN-PREPAID-001",
      issueDate: "2026-08-17",
      correctedInvoiceNumber: "INV-20260719-003",
      supplier: baseSupplier,
      customer: baseCustomer,
      lines: [{ id: "1", description: "Annulation", quantity: 1, unitPrice: 1000, taxPercent: 21, lineAmount: 1000 }],
      taxTotal: 0,
      taxableAmount: 1000,
      payableAmount: 0,
      prepaidAmount: 210,
    });
    expect(cn).toContain('<cbc:PrepaidAmount currencyID="EUR">210.00</cbc:PrepaidAmount>');
    expect(cn).toContain('<cbc:PayableAmount currencyID="EUR">1000.00</cbc:PayableAmount>');
  });
});

describe("BG-20 document level allowance", () => {
  const xml = generateInvoiceXml({
    ...invoiceBase,
    lines: [
      { id: "1", description: "Nourriture", quantity: 1, unitPrice: 9507, taxPercent: 6, lineAmount: 9507 },
      { id: "2", description: "Boissons", quantity: 1, unitPrice: 4381.57, taxPercent: 21, lineAmount: 4381.57 },
    ],
    taxableAmount: 13888.57,
    allowances: [
      { amount: 4107.5, taxPercent: 6, reason: "Acompte déjà facturé (6%)" },
      { amount: 1000, taxPercent: 21, reason: "Acompte déjà facturé (21%)" },
    ],
  });

  it("emits one AllowanceCharge per allowance with ChargeIndicator false", () => {
    expect(xml.match(/<cac:AllowanceCharge>/g)).toHaveLength(2);
    expect(xml.match(/<cbc:ChargeIndicator>false<\/cbc:ChargeIndicator>/g)).toHaveLength(2);
    expect(xml).toContain("<cbc:AllowanceChargeReason>Acompte déjà facturé (6%)</cbc:AllowanceChargeReason>");
  });

  it("sits between PaymentTerms/PaymentMeans and TaxTotal, as the UBL schema requires", () => {
    expect(xml.indexOf("<cac:AllowanceCharge>")).toBeLessThan(xml.indexOf("<cac:TaxTotal>"));
    expect(xml.indexOf("<cac:AllowanceCharge>")).toBeGreaterThan(xml.indexOf("<cac:AccountingCustomerParty>"));
  });

  it("reduces the taxable base globally (BR-CO-11/BR-CO-13)", () => {
    expect(xml).toContain('<cbc:LineExtensionAmount currencyID="EUR">13888.57</cbc:LineExtensionAmount>');
    expect(xml).toContain('<cbc:AllowanceTotalAmount currencyID="EUR">5107.50</cbc:AllowanceTotalAmount>');
    expect(xml).toContain('<cbc:TaxExclusiveAmount currencyID="EUR">8781.07</cbc:TaxExclusiveAmount>');
  });

  it("reduces the taxable base per VAT rate (BR-S-08)", () => {
    // 6%: 9507.00 − 4107.50 = 5399.50 → 323.97 ; 21%: 4381.57 − 1000 = 3381.57 → 710.13
    expect(xml).toContain('<cbc:TaxableAmount currencyID="EUR">5399.50</cbc:TaxableAmount>');
    expect(xml).toContain('<cbc:TaxableAmount currencyID="EUR">3381.57</cbc:TaxableAmount>');
    expect(extractAllTaxSubtotalAmounts(xml)).toEqual([323.97, 710.13]);
  });

  it("keeps BR-CO-15 (TaxInclusive = TaxExclusive + VAT) true", () => {
    const grab = (tag: string) => parseFloat(xml.match(new RegExp(`<cbc:${tag} currencyID="EUR">([-\\d.]+)`))![1]);
    expect(grab("TaxInclusiveAmount")).toBeCloseTo(grab("TaxExclusiveAmount") + 323.97 + 710.13, 2);
    expect(grab("PayableAmount")).toBe(grab("TaxInclusiveAmount"));
  });

  it("round-trips through parseUbl", () => {
    const parsed = parseUbl(xml);
    expect(parsed.allowanceCharges).toHaveLength(2);
    expect(parsed.allowanceCharges![0]).toMatchObject({ chargeIndicator: false, amount: 4107.5, taxCategory: "S", taxPercent: 6 });
    expect(parsed.monetaryTotal.allowanceTotalAmount).toBe(5107.5);
  });

  it("ignores a caller-provided gross total once allowances are in play", () => {
    // Old callers pass payableAmount as a hint; with BR-CO-13 in force the totals
    // are fully determined, so the hint must not override them.
    const hinted = generateInvoiceXml({
      ...invoiceBase,
      lines: [{ id: "1", description: "Service", quantity: 1, unitPrice: 1000, taxPercent: 21, lineAmount: 1000 }],
      taxableAmount: 1000,
      payableAmount: 1210,
      allowances: [{ amount: 100, taxPercent: 21, reasonCode: "95" }],
    });
    expect(hinted).toContain('<cbc:TaxExclusiveAmount currencyID="EUR">900.00</cbc:TaxExclusiveAmount>');
    expect(hinted).toContain('<cbc:TaxInclusiveAmount currencyID="EUR">1089.00</cbc:TaxInclusiveAmount>');
  });

  it("supports the percentage form (base amount + multiplier)", () => {
    const xmlPct = generateInvoiceXml({
      ...invoiceBase,
      lines: [{ id: "1", description: "Service", quantity: 1, unitPrice: 1000, taxPercent: 21, lineAmount: 1000 }],
      taxableAmount: 1000,
      allowances: [{ amount: 100, baseAmount: 1000, percent: 10, taxPercent: 21, reasonCode: "95", reason: "Discount" }],
    });
    expect(xmlPct).toContain("<cbc:MultiplierFactorNumeric>10</cbc:MultiplierFactorNumeric>");
    expect(xmlPct).toContain('<cbc:BaseAmount currencyID="EUR">1000.00</cbc:BaseAmount>');
    // AllowanceChargeType order: ReasonCode → Reason → Multiplier → Amount → BaseAmount → TaxCategory
    const block = xmlPct.match(/<cac:AllowanceCharge>[\s\S]*?<\/cac:AllowanceCharge>/)![0];
    const order = ["ChargeIndicator", "AllowanceChargeReasonCode", "AllowanceChargeReason", "MultiplierFactorNumeric", "Amount", "BaseAmount"];
    const positions = order.map((t) => block.indexOf(`<cbc:${t}`));
    expect(positions).toEqual([...positions].sort((a, b) => a - b));
    expect(block.indexOf("<cac:TaxCategory>")).toBeGreaterThan(block.indexOf("<cbc:BaseAmount"));
  });
});

describe("BG-21 document level charge", () => {
  const xml = generateInvoiceXml({
    ...invoiceBase,
    lines: [{ id: "1", description: "Goods", quantity: 1, unitPrice: 1000, taxPercent: 21, lineAmount: 1000 }],
    taxableAmount: 1000,
    charges: [{ amount: 50, taxPercent: 21, reason: "Freight" }],
  });

  it("adds to the taxable base with ChargeIndicator true (BR-CO-12/BR-CO-13)", () => {
    expect(xml).toContain("<cbc:ChargeIndicator>true</cbc:ChargeIndicator>");
    expect(xml).toContain('<cbc:ChargeTotalAmount currencyID="EUR">50.00</cbc:ChargeTotalAmount>');
    expect(xml).toContain('<cbc:TaxExclusiveAmount currencyID="EUR">1050.00</cbc:TaxExclusiveAmount>');
    expect(xml).toContain('<cbc:TaxableAmount currencyID="EUR">1050.00</cbc:TaxableAmount>');
    expect(xml).toContain('<cbc:PayableAmount currencyID="EUR">1270.50</cbc:PayableAmount>');
  });
});

describe("allowance/charge validation", () => {
  const singleLine = [{ id: "1", description: "Service", quantity: 1, unitPrice: 1000, taxPercent: 21, lineAmount: 1000 }];
  const build = (kind: "allowances" | "charges", item: any) => () =>
    generateInvoiceXml({ ...invoiceBase, lines: singleLine, taxableAmount: 1000, [kind]: [item] });

  // Every rejection carries the rule ID the Peppol validator would report, so a
  // user reading the error can look it up. Allowances and charges share the
  // checks but not the rule IDs (BG-20 vs BG-21).
  it.each<[string, any, RegExp]>([
    ["negative amount", { amount: -100, taxPercent: 21, reason: "Deposit" }, /BR-31/],
    ["zero amount", { amount: 0, taxPercent: 21, reason: "Deposit" }, /BR-31/],
    ["amount that is not a number", { amount: NaN, taxPercent: 21, reason: "Deposit" }, /BR-31/],
    ["no reason and no reason code", { amount: 100, taxPercent: 21 }, /BR-33/],
    ["blank reason", { amount: 100, taxPercent: 21, reason: "   " }, /BR-33/],
    ["category S at a zero rate", { amount: 100, taxPercent: 0, taxCategory: "S", reason: "D" }, /BR-S-06/],
    ["base amount without a percentage", { amount: 100, baseAmount: 1000, taxPercent: 21, reason: "D" }, /R041\/R042/],
    ["percentage without a base amount", { amount: 100, percent: 10, taxPercent: 21, reason: "D" }, /R041\/R042/],
    ["amount contradicting base × percentage", { amount: 250, baseAmount: 1000, percent: 10, taxPercent: 21, reason: "D" }, /R040/],
    ["exempt category at a nonzero rate", { amount: 100, taxPercent: 21, taxCategory: "E", reason: "D" }, /"E".*zero/],
    ["reverse-charge category at a nonzero rate", { amount: 100, taxPercent: 6, taxCategory: "AE", reason: "D" }, /"AE".*zero/],
    ["VAT rate that is not a number", { amount: 100, taxPercent: NaN, reason: "D" }, /invalid VAT rate/],
  ])("refuses an allowance with a %s", (_what, item, rule) => {
    expect(build("allowances", item)).toThrow(rule);
  });

  it.each<[string, any, RegExp]>([
    ["negative amount", { amount: -50, taxPercent: 21, reason: "Freight" }, /BR-36/],
    ["no reason and no reason code", { amount: 50, taxPercent: 21 }, /BR-38/],
    ["category S at a zero rate", { amount: 50, taxPercent: 0, taxCategory: "S", reason: "Freight" }, /BR-S-06/],
  ])("refuses a charge with a %s, using the BG-21 rule IDs", (_what, item, rule) => {
    expect(build("charges", item)).toThrow(rule);
  });

  it.each<[string, any]>([
    ["a reason code instead of a reason", { amount: 100, taxPercent: 21, reasonCode: "95" }],
    ["an exempt category at a zero rate", { amount: 100, taxPercent: 0, taxCategory: "E", reason: "D" }],
    ["base × percentage within the one-cent tolerance", { amount: 100.02, baseAmount: 1000, percent: 10, taxPercent: 21, reason: "D" }],
  ])("accepts an allowance with %s", (_what, item) => {
    expect(build("allowances", item)).not.toThrow();
  });

  it("stops accepting base × percentage drift past the tolerance (R040)", () => {
    expect(build("allowances", { amount: 100.03, baseAmount: 1000, percent: 10, taxPercent: 21, reason: "D" })).toThrow(/R040/);
  });

  // BR-CO-11: AllowanceTotalAmount = Σ of the amounts as written. Two amounts of
  // 0.125 are written as 0.13 each, so the total is 0.26 and not the 0.25 a sum
  // of the raw values would give.
  it("sums the amounts as they are written in the document (BR-CO-11)", () => {
    const xml = generateInvoiceXml({
      ...invoiceBase,
      lines: singleLine,
      taxableAmount: 1000,
      allowances: [
        { amount: 0.125, taxPercent: 21, reason: "Rebate A" },
        { amount: 0.125, taxPercent: 21, reason: "Rebate B" },
      ],
    });
    expect(xml.match(/<cbc:Amount currencyID="EUR">([\d.]+)<\/cbc:Amount>/g))
      .toEqual(['<cbc:Amount currencyID="EUR">0.13</cbc:Amount>', '<cbc:Amount currencyID="EUR">0.13</cbc:Amount>']);
    expect(xml).toContain("<cbc:AllowanceTotalAmount currencyID=\"EUR\">0.26</cbc:AllowanceTotalAmount>");
  });

  it("omits the rate on an out-of-scope allowance (BR-O-05)", () => {
    const xml = generateInvoiceXml({
      ...invoiceBase,
      lines: [{ id: "1", description: "Out of scope", quantity: 1, unitPrice: 1000, taxPercent: 0, taxCategory: "O", lineAmount: 1000 }],
      taxableAmount: 1000,
      allowances: [{ amount: 100, taxPercent: 0, taxCategory: "O", reason: "Rebate" }],
    });
    const allowanceEl = xml.match(/<cac:AllowanceCharge>[\s\S]*?<\/cac:AllowanceCharge>/)![0];
    expect(allowanceEl).toContain("<cbc:ID>O</cbc:ID>");
    expect(allowanceEl).not.toContain("cbc:Percent");
  });
});

// ─── Coverage of the remaining generator decisions ────────────────────────────

describe("BR-16 — a document without lines is refused", () => {
  it.each([
    ["invoice", () => generateInvoiceXml({ ...invoiceBase, lines: [] })],
    ["credit note", () => generateCreditNoteXml({
      creditNoteNumber: "CN-EMPTY-001", issueDate: "2026-08-17", correctedInvoiceNumber: "INV-1",
      supplier: baseSupplier, customer: baseCustomer, lines: [], taxTotal: 0, taxableAmount: 0, payableAmount: 0,
    })],
  ])("throws for an empty %s instead of emitting a document nobody accepts", (_kind, build) => {
    expect(build).toThrow(/BR-16/);
  });
});

describe("zero-rated lines (VAT category E)", () => {
  const xml = generateInvoiceXml({
    ...invoiceBase,
    lines: [{ id: "1", description: "Exempt service", quantity: 1, unitPrice: 100, taxPercent: 0, lineAmount: 100 }],
    taxableAmount: 100,
  });

  it("defaults the category to E and carries an exemption reason (BR-E-*)", () => {
    expect(xml).toContain("<cbc:ID>E</cbc:ID>");
    expect(xml).toContain("<cbc:TaxExemptionReasonCode>vatex-eu-132</cbc:TaxExemptionReasonCode>");
    expect(xml).toContain('<cbc:TaxAmount currencyID="EUR">0.00</cbc:TaxAmount>');
    expect(xml).toContain('<cbc:TaxInclusiveAmount currencyID="EUR">100.00</cbc:TaxInclusiveAmount>');
  });

  it("emits no exemption reason for a standard-rated line", () => {
    const standard = generateInvoiceXml({
      ...invoiceBase,
      lines: [{ id: "1", description: "Service", quantity: 1, unitPrice: 100, taxPercent: 21, lineAmount: 100 }],
      taxableAmount: 100,
    });
    expect(standard).toContain("<cbc:ID>S</cbc:ID>");
    expect(standard).not.toContain("TaxExemptionReason");
  });

  it("keeps an explicit category over the rate-based default", () => {
    const reverseCharge = generateInvoiceXml({
      ...invoiceBase,
      lines: [{ id: "1", description: "Service", quantity: 1, unitPrice: 100, taxPercent: 0, taxCategory: "AE", lineAmount: 100 }],
      taxableAmount: 100,
    });
    expect(reverseCharge).toContain("<cbc:ID>AE</cbc:ID>");
    expect(reverseCharge).not.toContain("TaxExemptionReason");
  });
});

describe("authoritative tax total across several rates", () => {
  // Subtraction-method callers know the total VAT but not its split. The residual
  // must land on the largest taxed base, never silently break BR-CO-14 — and it
  // must stay inside the ±1 tolerance BR-S-09 allows per category.
  const twoRates = (over: Record<string, unknown>) => () => generateInvoiceXml({
    ...invoiceBase,
    lines: [
      { id: "1", description: "Small 21%", quantity: 1, unitPrice: 100, taxPercent: 21, lineAmount: 100 },
      { id: "2", description: "Large 6%", quantity: 1, unitPrice: 1000, taxPercent: 6, lineAmount: 1000 },
    ],
    taxableAmount: 1100,
    ...over,
  });

  const xml = twoRates({ taxTotal: 80.6 })();

  it("pushes the residual onto the largest taxable base, not the first one", () => {
    expect(extractAllTaxSubtotalAmounts(xml)).toEqual([21, 59.6]);
  });

  it("keeps the breakdown adding up to the authoritative total (BR-CO-14)", () => {
    const total = parseFloat(xml.match(/<cac:TaxTotal>\s*<cbc:TaxAmount[^>]*>([\d.]+)</)![1]);
    expect(total).toBe(80.6);
    expect(extractAllTaxSubtotalAmounts(xml).reduce((s, a) => s + a, 0)).toBe(total);
  });

  it("refuses a difference of a whole currency unit (BR-S-09)", () => {
    expect(twoRates({ taxTotal: 80 })).toThrow(/BR-S-09/);
  });

  it("refuses to put VAT on a document where every category is zero-rated", () => {
    const exempt = () => generateInvoiceXml({
      ...invoiceBase,
      lines: [{ id: "1", description: "Exempt", quantity: 1, unitPrice: 100, taxPercent: 0, lineAmount: 100 }],
      taxableAmount: 100,
      taxTotal: 0.5,
    });
    expect(exempt).toThrow(/BR-E-09/);
  });

  it("ignores the hint once a discount is in play, so the breakdown stays exact", () => {
    const withAllowance = twoRates({
      taxTotal: 80.6,
      allowances: [{ amount: 100, taxPercent: 6, reason: "Deposit" }],
    })();
    // 21% on 100 + 6% on 900 — the hint computed on the undiscounted base is dropped.
    expect(extractAllTaxSubtotalAmounts(withAllowance)).toEqual([21, 54]);
  });
});

describe("per-rate reconciliation of allowances and charges (BR-S-08)", () => {
  const line21 = { id: "1", description: "Service", quantity: 1, unitPrice: 1000, taxPercent: 21, lineAmount: 1000 };
  const build = (over: Record<string, unknown>) =>
    generateInvoiceXml({ ...invoiceBase, lines: [line21], taxableAmount: 1000, ...over });

  it("merges several allowances at the same rate into one subtotal", () => {
    const xml = build({
      allowances: [
        { amount: 100, taxPercent: 21, reason: "Deposit" },
        { amount: 50, taxPercent: 21, reason: "Loyalty" },
      ],
    });
    expect(xml.match(/<cac:TaxSubtotal>/g)).toHaveLength(1);
    expect(xml).toContain('<cbc:TaxableAmount currencyID="EUR">850.00</cbc:TaxableAmount>');
    expect(xml).toContain('<cbc:AllowanceTotalAmount currencyID="EUR">150.00</cbc:AllowanceTotalAmount>');
    expect(extractAllTaxSubtotalAmounts(xml)).toEqual([178.5]);
  });

  it("opens a subtotal for a rate that only an allowance uses", () => {
    const xml = build({ allowances: [{ amount: 100, taxPercent: 6, reason: "Deposit at 6%" }] });
    expect(extractAllTaxSubtotalAmounts(xml)).toEqual([210, -6]);
    expect(xml).toContain('<cbc:TaxableAmount currencyID="EUR">-100.00</cbc:TaxableAmount>');
    expect(xml).toContain('<cbc:TaxExclusiveAmount currencyID="EUR">900.00</cbc:TaxExclusiveAmount>');
  });

  it("nets an allowance and a charge on the same rate", () => {
    const xml = build({
      allowances: [{ amount: 200, taxPercent: 21, reason: "Deposit" }],
      charges: [{ amount: 50, taxPercent: 21, reason: "Freight" }],
    });
    expect(xml).toContain('<cbc:TaxableAmount currencyID="EUR">850.00</cbc:TaxableAmount>');
    expect(xml).toContain('<cbc:TaxExclusiveAmount currencyID="EUR">850.00</cbc:TaxExclusiveAmount>');
    expect(xml).toContain('<cbc:PayableAmount currencyID="EUR">1028.50</cbc:PayableAmount>');
  });

  it("orders AllowanceTotalAmount before ChargeTotalAmount before PrepaidAmount", () => {
    const xml = build({
      allowances: [{ amount: 200, taxPercent: 21, reason: "Deposit" }],
      charges: [{ amount: 50, taxPercent: 21, reason: "Freight" }],
      prepaidAmount: 28.5,
    });
    const order = ["LineExtensionAmount", "TaxExclusiveAmount", "TaxInclusiveAmount", "AllowanceTotalAmount", "ChargeTotalAmount", "PrepaidAmount", "PayableAmount"];
    const from = xml.indexOf("<cac:LegalMonetaryTotal>");
    const positions = order.map((tag) => xml.indexOf(`<cbc:${tag} currencyID="EUR"`, from));
    expect(positions.filter((p) => p === -1)).toEqual([]);
    expect(positions).toEqual([...positions].sort((a, b) => a - b));
    expect(xml).toContain('<cbc:PayableAmount currencyID="EUR">1000.00</cbc:PayableAmount>');
  });

  it("emits allowances before charges, each in its own AllowanceCharge", () => {
    const xml = build({
      allowances: [{ amount: 200, taxPercent: 21, reason: "Deposit" }],
      charges: [{ amount: 50, taxPercent: 21, reason: "Freight" }],
    });
    expect(xml.indexOf("<cbc:ChargeIndicator>false")).toBeLessThan(xml.indexOf("<cbc:ChargeIndicator>true"));
    expect(xml.match(/<cac:AllowanceCharge>/g)).toHaveLength(2);
  });

  it("ignores a gross-total hint once an amount is already paid", () => {
    // payableAmount is a legacy hint; with BT-113 in play BR-CO-16 fixes the totals.
    const xml = build({ payableAmount: 1500, prepaidAmount: 210 });
    expect(xml).toContain('<cbc:TaxInclusiveAmount currencyID="EUR">1210.00</cbc:TaxInclusiveAmount>');
    expect(xml).toContain('<cbc:PayableAmount currencyID="EUR">1000.00</cbc:PayableAmount>');
  });

  it("reports a negative payable when more was paid than is owed", () => {
    const xml = build({ prepaidAmount: 2000 });
    expect(xml).toContain('<cbc:PayableAmount currencyID="EUR">-790.00</cbc:PayableAmount>');
  });
});

describe("party identifiers", () => {
  const build = (party: Partial<typeof baseSupplier>) => generateInvoiceXml({
    ...invoiceBase,
    supplier: { ...baseSupplier, ...party },
    lines: [{ id: "1", description: "Service", quantity: 1, unitPrice: 100, taxPercent: 21, lineAmount: 100 }],
    taxableAmount: 100,
  });

  it("splits scheme and identifier, dropping the country prefix of the identifier", () => {
    expect(build({ peppolId: "0208:BE1034898146" }))
      .toContain('<cbc:EndpointID schemeID="0208">1034898146</cbc:EndpointID>');
  });

  it("falls back to scheme 0208 for a bare identifier, never emits it as a scheme", () => {
    const xml = build({ peppolId: "1034898146" });
    expect(xml).toContain('<cbc:EndpointID schemeID="0208">1034898146</cbc:EndpointID>');
    expect(xml).not.toContain('schemeID="1034898146"');
  });

  it("takes a non-default scheme from the caller", () => {
    expect(build({ peppolId: "9925:BE0794263219" })).toContain('schemeID="9925"');
  });

  it.each([
    ["already prefixed", "BE1034898146", "BE"],
    ["unprefixed, prefixed from the country", "1034898146", "BE"],
    ["punctuated", "be 1034.898.146", "BE"],
    ["prefixed with another country than the address", "NL123456789B01", "BE"],
  ])("normalises the %s VAT number", (_what, vatNumber, countryCode) => {
    const xml = build({ vatNumber, countryCode });
    const companyId = xml.match(/<cac:PartyTaxScheme>\s*<cbc:CompanyID>([^<]+)</)![1];
    expect(companyId, `vatNumber ${vatNumber} / country ${countryCode}`).toMatch(/^[A-Z]{2}[0-9A-Z]+$/);
    expect(companyId).not.toMatch(/[^0-9A-Z]/);
  });
});

describe("invoice document structure", () => {
  const line = { id: "1", description: "Service", quantity: 1, unitPrice: 100, taxPercent: 21, lineAmount: 100 };
  const build = (over: Record<string, unknown> = {}) =>
    generateInvoiceXml({ ...invoiceBase, lines: [line], taxableAmount: 100, ...over });

  it("keeps the UBL Invoice element order for optional references", () => {
    const xml = build({
      orderReference: "PO-123",
      originatorReference: "ORIG-789",
      contractReference: "CTR-456",
      documentReferences: [{ id: "ATT-1" }],
      projectReference: "PRJ-1",
      paymentInfo: { iban: "BE68539007547034" },
      allowances: [{ amount: 10, taxPercent: 21, reason: "Discount" }],
    });
    const order = [
      "<cbc:BuyerReference>", "PO-123", "ORIG-789", "CTR-456", "ATT-1", "PRJ-1",
      "<cac:AccountingSupplierParty>", "<cac:AccountingCustomerParty>", "<cac:PaymentMeans>",
      "<cac:AllowanceCharge>", "<cac:TaxTotal>", "<cac:LegalMonetaryTotal>", "<cac:InvoiceLine>",
    ];
    const positions = order.map((needle) => xml.indexOf(needle));
    expect(order.filter((_, i) => positions[i] === -1)).toEqual([]);
    expect(positions).toEqual([...positions].sort((a, b) => a - b));
  });

  it("defaults the buyer reference to the invoice number (BT-10 must not be empty)", () => {
    expect(build()).toContain("<cbc:BuyerReference>INV-ALLOW-001</cbc:BuyerReference>");
    expect(build({ buyerReference: "BR-42" })).toContain("<cbc:BuyerReference>BR-42</cbc:BuyerReference>");
  });

  it("defaults the unit of measure to C62 (one unit) and keeps a given one", () => {
    expect(build()).toContain('<cbc:InvoicedQuantity unitCode="C62">1</cbc:InvoicedQuantity>');
    expect(build({ lines: [{ ...line, unitCode: "HUR" }] })).toContain('unitCode="HUR"');
  });

  it("embeds an attachment inside its AdditionalDocumentReference", () => {
    const xml = build({
      documentReferences: [{
        id: "ATT-1", typeCode: "130", description: "Timesheet",
        attachment: { base64Content: "SGVsbG8=", mimeCode: "application/pdf", filename: "sheet.pdf" },
      }],
    });
    expect(xml).toMatch(
      /<cac:AdditionalDocumentReference>\s*<cbc:ID>ATT-1<\/cbc:ID>\s*<cbc:DocumentTypeCode>130<\/cbc:DocumentTypeCode>\s*<cbc:DocumentDescription>Timesheet<\/cbc:DocumentDescription>\s*<cac:Attachment>\s*<cbc:EmbeddedDocumentBinaryObject mimeCode="application\/pdf" filename="sheet\.pdf">SGVsbG8=</,
    );
  });

  it("omits every optional block that was not asked for", () => {
    const xml = build();
    for (const tag of ["cac:OrderReference", "cac:ContractDocumentReference", "cac:ProjectReference", "cac:OriginatorDocumentReference", "cac:AdditionalDocumentReference", "cac:PaymentMeans", "cac:AllowanceCharge", "cbc:Note", "cbc:TaxCurrencyCode", "cac:Contact"]) {
      expect(xml, `${tag} must be omitted when unused`).not.toContain(`<${tag}>`);
    }
  });

  it("emits the customer contact block as soon as one contact field is known", () => {
    const xml = build({ customer: { ...baseCustomer, contactEmail: "a@b.c" } });
    expect(xml).toContain("<cbc:ElectronicMail>a@b.c</cbc:ElectronicMail>");
    expect(xml).not.toContain("<cbc:Telephone>");
  });

  it("accepts a tax currency equal to the document currency without a converted amount", () => {
    const xml = build({ currency: "EUR", taxCurrency: "EUR" });
    expect(xml).not.toContain("TaxCurrencyCode");
    expect(extractTaxTotals(xml)).toHaveLength(1);
  });
});
