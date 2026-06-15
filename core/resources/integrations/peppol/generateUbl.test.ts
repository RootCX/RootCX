import { describe, it, expect } from "vitest";
import { generateInvoiceXml, generateCreditNoteXml } from "./generateUbl";

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
  const re = /<cac:TaxSubtotal>[\s\S]*?<cbc:TaxAmount[^>]*>([\d.]+)<\/cbc:TaxAmount>[\s\S]*?<\/cac:TaxSubtotal>/g;
  let m;
  while ((m = re.exec(xml)) !== null) amounts.push(parseFloat(m[1]));
  return amounts;
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
