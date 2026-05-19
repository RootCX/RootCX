import { describe, it, expect } from "vitest";
import { generateInvoiceXml } from "./generateUbl";

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
