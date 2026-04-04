import { describe, it, expect } from "vitest";
import { parseUbl } from "./parseUbl";

// ─── Fixtures ───────────────────────────────────────────────────────────────

/** Exercises every field in the Peppol BIS 3.0 Invoice syntax tree */
const FULL_SPEC_INVOICE = `<?xml version="1.0" encoding="UTF-8"?>
<Invoice xmlns="urn:oasis:names:specification:ubl:schema:xsd:Invoice-2"
         xmlns:cac="urn:oasis:names:specification:ubl:schema:xsd:CommonAggregateComponents-2"
         xmlns:cbc="urn:oasis:names:specification:ubl:schema:xsd:CommonBasicComponents-2">
    <cbc:CustomizationID>urn:cen.eu:en16931:2017#compliant#urn:fdc:peppol.eu:2017:poacc:billing:3.0</cbc:CustomizationID>
    <cbc:ProfileID>urn:fdc:peppol.eu:2017:poacc:billing:01:1.0</cbc:ProfileID>
    <cbc:ID>INV-2024-001</cbc:ID>
    <cbc:IssueDate>2024-03-15</cbc:IssueDate>
    <cbc:DueDate>2024-04-15</cbc:DueDate>
    <cbc:InvoiceTypeCode>380</cbc:InvoiceTypeCode>
    <cbc:Note>Payment within 30 days</cbc:Note>
    <cbc:TaxPointDate>2024-03-15</cbc:TaxPointDate>
    <cbc:DocumentCurrencyCode>EUR</cbc:DocumentCurrencyCode>
    <cbc:TaxCurrencyCode>USD</cbc:TaxCurrencyCode>
    <cbc:AccountingCost>4217:01:FA</cbc:AccountingCost>
    <cbc:BuyerReference>PO-2024-42</cbc:BuyerReference>
    <cac:InvoicePeriod>
        <cbc:StartDate>2024-03-01</cbc:StartDate>
        <cbc:EndDate>2024-03-31</cbc:EndDate>
        <cbc:DescriptionCode>35</cbc:DescriptionCode>
    </cac:InvoicePeriod>
    <cac:OrderReference>
        <cbc:ID>ORD-99</cbc:ID>
        <cbc:SalesOrderID>SO-123</cbc:SalesOrderID>
    </cac:OrderReference>
    <cac:BillingReference>
        <cac:InvoiceDocumentReference>
            <cbc:ID>INV-2023-100</cbc:ID>
            <cbc:IssueDate>2023-12-01</cbc:IssueDate>
        </cac:InvoiceDocumentReference>
    </cac:BillingReference>
    <cac:DespatchDocumentReference><cbc:ID>DESP-001</cbc:ID></cac:DespatchDocumentReference>
    <cac:ReceiptDocumentReference><cbc:ID>REC-001</cbc:ID></cac:ReceiptDocumentReference>
    <cac:OriginatorDocumentReference><cbc:ID>ORIG-001</cbc:ID></cac:OriginatorDocumentReference>
    <cac:ContractDocumentReference><cbc:ID>CTR-55</cbc:ID></cac:ContractDocumentReference>
    <cac:AdditionalDocumentReference>
        <cbc:ID>ATT-1</cbc:ID>
        <cbc:DocumentDescription>Timesheet</cbc:DocumentDescription>
        <cac:Attachment>
            <cbc:EmbeddedDocumentBinaryObject mimeCode="application/pdf" filename="timesheet.pdf">dGVzdA==</cbc:EmbeddedDocumentBinaryObject>
        </cac:Attachment>
    </cac:AdditionalDocumentReference>
    <cac:AdditionalDocumentReference>
        <cbc:ID>ATT-2</cbc:ID>
        <cac:Attachment>
            <cac:ExternalReference><cbc:URI>https://example.com/doc.pdf</cbc:URI></cac:ExternalReference>
        </cac:Attachment>
    </cac:AdditionalDocumentReference>
    <cac:ProjectReference><cbc:ID>PRJ-7</cbc:ID></cac:ProjectReference>
    <cac:AccountingSupplierParty>
        <cac:Party>
            <cbc:EndpointID schemeID="0208">0123456789</cbc:EndpointID>
            <cac:PartyIdentification><cbc:ID schemeID="0208">0123456789</cbc:ID></cac:PartyIdentification>
            <cac:PartyName><cbc:Name>Acme Corp</cbc:Name></cac:PartyName>
            <cac:PostalAddress>
                <cbc:StreetName>Rue de la Loi 1</cbc:StreetName>
                <cbc:AdditionalStreetName>Building B</cbc:AdditionalStreetName>
                <cbc:CityName>Brussels</cbc:CityName>
                <cbc:PostalZone>1000</cbc:PostalZone>
                <cbc:CountrySubentity>BRU</cbc:CountrySubentity>
                <cac:AddressLine><cbc:Line>Floor 5</cbc:Line></cac:AddressLine>
                <cac:Country><cbc:IdentificationCode>BE</cbc:IdentificationCode></cac:Country>
            </cac:PostalAddress>
            <cac:PartyTaxScheme>
                <cbc:CompanyID>BE0123456789</cbc:CompanyID>
                <cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme>
            </cac:PartyTaxScheme>
            <cac:PartyLegalEntity>
                <cbc:RegistrationName>Acme Corp</cbc:RegistrationName>
                <cbc:CompanyID schemeID="0208">0123456789</cbc:CompanyID>
                <cbc:CompanyLegalForm>SA</cbc:CompanyLegalForm>
            </cac:PartyLegalEntity>
            <cac:Contact>
                <cbc:Name>John Doe</cbc:Name>
                <cbc:Telephone>+32 2 123 4567</cbc:Telephone>
                <cbc:ElectronicMail>john@acme.be</cbc:ElectronicMail>
            </cac:Contact>
        </cac:Party>
    </cac:AccountingSupplierParty>
    <cac:AccountingCustomerParty>
        <cac:Party>
            <cbc:EndpointID schemeID="0208">9876543210</cbc:EndpointID>
            <cac:PartyIdentification><cbc:ID schemeID="0208">9876543210</cbc:ID></cac:PartyIdentification>
            <cac:PartyName><cbc:Name>Beta NV</cbc:Name></cac:PartyName>
            <cac:PostalAddress>
                <cbc:StreetName>Avenue Louise 42</cbc:StreetName>
                <cbc:CityName>Antwerp</cbc:CityName>
                <cbc:PostalZone>2000</cbc:PostalZone>
                <cac:Country><cbc:IdentificationCode>BE</cbc:IdentificationCode></cac:Country>
            </cac:PostalAddress>
            <cac:PartyTaxScheme>
                <cbc:CompanyID>BE9876543210</cbc:CompanyID>
                <cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme>
            </cac:PartyTaxScheme>
            <cac:PartyLegalEntity>
                <cbc:RegistrationName>Beta NV</cbc:RegistrationName>
                <cbc:CompanyID schemeID="0208">9876543210</cbc:CompanyID>
            </cac:PartyLegalEntity>
            <cac:Contact>
                <cbc:Name>Jane Smith</cbc:Name>
                <cbc:ElectronicMail>jane@beta.be</cbc:ElectronicMail>
            </cac:Contact>
        </cac:Party>
    </cac:AccountingCustomerParty>
    <cac:PayeeParty>
        <cac:PartyIdentification><cbc:ID schemeID="0208">5555555555</cbc:ID></cac:PartyIdentification>
        <cac:PartyName><cbc:Name>Factoring Co</cbc:Name></cac:PartyName>
        <cac:PartyLegalEntity><cbc:CompanyID schemeID="0208">5555555555</cbc:CompanyID></cac:PartyLegalEntity>
    </cac:PayeeParty>
    <cac:TaxRepresentativeParty>
        <cac:PartyName><cbc:Name>Tax Rep GmbH</cbc:Name></cac:PartyName>
        <cac:PostalAddress>
            <cbc:StreetName>Friedrichstraße 10</cbc:StreetName>
            <cbc:CityName>Berlin</cbc:CityName>
            <cbc:PostalZone>10117</cbc:PostalZone>
            <cac:Country><cbc:IdentificationCode>DE</cbc:IdentificationCode></cac:Country>
        </cac:PostalAddress>
        <cac:PartyTaxScheme>
            <cbc:CompanyID>DE123456789</cbc:CompanyID>
            <cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme>
        </cac:PartyTaxScheme>
    </cac:TaxRepresentativeParty>
    <cac:Delivery>
        <cbc:ActualDeliveryDate>2024-03-20</cbc:ActualDeliveryDate>
        <cac:DeliveryLocation>
            <cbc:ID schemeID="0088">5790000435944</cbc:ID>
            <cac:Address>
                <cbc:StreetName>Delivery Street 1</cbc:StreetName>
                <cbc:CityName>Ghent</cbc:CityName>
                <cbc:PostalZone>9000</cbc:PostalZone>
                <cac:Country><cbc:IdentificationCode>BE</cbc:IdentificationCode></cac:Country>
            </cac:Address>
        </cac:DeliveryLocation>
        <cac:DeliveryParty>
            <cac:PartyName><cbc:Name>Warehouse Beta</cbc:Name></cac:PartyName>
        </cac:DeliveryParty>
    </cac:Delivery>
    <cac:PaymentMeans>
        <cbc:PaymentMeansCode name="Credit transfer">30</cbc:PaymentMeansCode>
        <cbc:PaymentID>INV-2024-001</cbc:PaymentID>
        <cac:PayeeFinancialAccount>
            <cbc:ID>BE68539007547034</cbc:ID>
            <cbc:Name>Acme Corp</cbc:Name>
            <cac:FinancialInstitutionBranch>
                <cbc:ID>BBRUBEBB</cbc:ID>
            </cac:FinancialInstitutionBranch>
        </cac:PayeeFinancialAccount>
    </cac:PaymentMeans>
    <cac:PaymentMeans>
        <cbc:PaymentMeansCode>48</cbc:PaymentMeansCode>
        <cac:CardAccount>
            <cbc:PrimaryAccountNumberID>1234</cbc:PrimaryAccountNumberID>
            <cbc:NetworkID>VISA</cbc:NetworkID>
            <cbc:HolderName>John Doe</cbc:HolderName>
        </cac:CardAccount>
    </cac:PaymentMeans>
    <cac:PaymentMeans>
        <cbc:PaymentMeansCode>49</cbc:PaymentMeansCode>
        <cac:PaymentMandate>
            <cbc:ID>MANDATE-001</cbc:ID>
            <cac:PayerFinancialAccount><cbc:ID>BE68000000001234</cbc:ID></cac:PayerFinancialAccount>
        </cac:PaymentMandate>
    </cac:PaymentMeans>
    <cac:PaymentTerms><cbc:Note>Net 30 days, 2% discount if paid within 10 days</cbc:Note></cac:PaymentTerms>
    <cac:AllowanceCharge>
        <cbc:ChargeIndicator>false</cbc:ChargeIndicator>
        <cbc:AllowanceChargeReasonCode>95</cbc:AllowanceChargeReasonCode>
        <cbc:AllowanceChargeReason>Discount</cbc:AllowanceChargeReason>
        <cbc:MultiplierFactorNumeric>10</cbc:MultiplierFactorNumeric>
        <cbc:Amount currencyID="EUR">100.00</cbc:Amount>
        <cbc:BaseAmount currencyID="EUR">1000.00</cbc:BaseAmount>
        <cac:TaxCategory>
            <cbc:ID>S</cbc:ID>
            <cbc:Percent>21</cbc:Percent>
            <cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme>
        </cac:TaxCategory>
    </cac:AllowanceCharge>
    <cac:AllowanceCharge>
        <cbc:ChargeIndicator>true</cbc:ChargeIndicator>
        <cbc:AllowanceChargeReason>Freight</cbc:AllowanceChargeReason>
        <cbc:Amount currencyID="EUR">50.00</cbc:Amount>
        <cac:TaxCategory>
            <cbc:ID>S</cbc:ID>
            <cbc:Percent>21</cbc:Percent>
            <cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme>
        </cac:TaxCategory>
    </cac:AllowanceCharge>
    <cac:TaxTotal>
        <cbc:TaxAmount currencyID="EUR">199.50</cbc:TaxAmount>
        <cac:TaxSubtotal>
            <cbc:TaxableAmount currencyID="EUR">950.00</cbc:TaxableAmount>
            <cbc:TaxAmount currencyID="EUR">199.50</cbc:TaxAmount>
            <cac:TaxCategory>
                <cbc:ID>S</cbc:ID>
                <cbc:Percent>21</cbc:Percent>
                <cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme>
            </cac:TaxCategory>
        </cac:TaxSubtotal>
    </cac:TaxTotal>
    <cac:TaxTotal>
        <cbc:TaxAmount currencyID="USD">215.00</cbc:TaxAmount>
    </cac:TaxTotal>
    <cac:LegalMonetaryTotal>
        <cbc:LineExtensionAmount currencyID="EUR">1000.00</cbc:LineExtensionAmount>
        <cbc:TaxExclusiveAmount currencyID="EUR">950.00</cbc:TaxExclusiveAmount>
        <cbc:TaxInclusiveAmount currencyID="EUR">1149.50</cbc:TaxInclusiveAmount>
        <cbc:AllowanceTotalAmount currencyID="EUR">100.00</cbc:AllowanceTotalAmount>
        <cbc:ChargeTotalAmount currencyID="EUR">50.00</cbc:ChargeTotalAmount>
        <cbc:PrepaidAmount currencyID="EUR">200.00</cbc:PrepaidAmount>
        <cbc:PayableRoundingAmount currencyID="EUR">0.50</cbc:PayableRoundingAmount>
        <cbc:PayableAmount currencyID="EUR">950.00</cbc:PayableAmount>
    </cac:LegalMonetaryTotal>
    <cac:InvoiceLine>
        <cbc:ID>1</cbc:ID>
        <cbc:Note>First delivery batch</cbc:Note>
        <cbc:InvoicedQuantity unitCode="HUR">10</cbc:InvoicedQuantity>
        <cbc:LineExtensionAmount currencyID="EUR">500.00</cbc:LineExtensionAmount>
        <cbc:AccountingCost>1234:56</cbc:AccountingCost>
        <cac:InvoicePeriod>
            <cbc:StartDate>2024-03-01</cbc:StartDate>
            <cbc:EndDate>2024-03-15</cbc:EndDate>
        </cac:InvoicePeriod>
        <cac:OrderLineReference><cbc:LineID>OL-1</cbc:LineID></cac:OrderLineReference>
        <cac:DocumentReference>
            <cbc:ID schemeID="ABZ">DOC-REF-1</cbc:ID>
            <cbc:DocumentTypeCode>130</cbc:DocumentTypeCode>
        </cac:DocumentReference>
        <cac:AllowanceCharge>
            <cbc:ChargeIndicator>false</cbc:ChargeIndicator>
            <cbc:AllowanceChargeReasonCode>95</cbc:AllowanceChargeReasonCode>
            <cbc:AllowanceChargeReason>Line discount</cbc:AllowanceChargeReason>
            <cbc:Amount currencyID="EUR">25.00</cbc:Amount>
        </cac:AllowanceCharge>
        <cac:Item>
            <cbc:Description>Expert consulting services for Q1</cbc:Description>
            <cbc:Name>Consulting hours</cbc:Name>
            <cac:BuyersItemIdentification><cbc:ID>BUY-001</cbc:ID></cac:BuyersItemIdentification>
            <cac:SellersItemIdentification><cbc:ID>SEL-001</cbc:ID></cac:SellersItemIdentification>
            <cac:StandardItemIdentification><cbc:ID schemeID="0160">1234567890128</cbc:ID></cac:StandardItemIdentification>
            <cac:OriginCountry><cbc:IdentificationCode>BE</cbc:IdentificationCode></cac:OriginCountry>
            <cac:CommodityClassification>
                <cbc:ItemClassificationCode listID="STI" listVersionID="2">72212000</cbc:ItemClassificationCode>
            </cac:CommodityClassification>
            <cac:ClassifiedTaxCategory>
                <cbc:ID>S</cbc:ID>
                <cbc:Percent>21</cbc:Percent>
                <cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme>
            </cac:ClassifiedTaxCategory>
            <cac:AdditionalItemProperty>
                <cbc:Name>Color</cbc:Name>
                <cbc:Value>Blue</cbc:Value>
            </cac:AdditionalItemProperty>
        </cac:Item>
        <cac:Price>
            <cbc:PriceAmount currencyID="EUR">50.00</cbc:PriceAmount>
            <cbc:BaseQuantity unitCode="HUR">1</cbc:BaseQuantity>
            <cac:AllowanceCharge>
                <cbc:ChargeIndicator>false</cbc:ChargeIndicator>
                <cbc:Amount currencyID="EUR">5.00</cbc:Amount>
                <cbc:BaseAmount currencyID="EUR">55.00</cbc:BaseAmount>
            </cac:AllowanceCharge>
        </cac:Price>
    </cac:InvoiceLine>
    <cac:InvoiceLine>
        <cbc:ID>2</cbc:ID>
        <cbc:InvoicedQuantity unitCode="C62">5</cbc:InvoicedQuantity>
        <cbc:LineExtensionAmount currencyID="EUR">500.00</cbc:LineExtensionAmount>
        <cac:Item>
            <cbc:Name>Software licenses</cbc:Name>
            <cac:SellersItemIdentification><cbc:ID>SEL-002</cbc:ID></cac:SellersItemIdentification>
            <cac:ClassifiedTaxCategory>
                <cbc:ID>S</cbc:ID>
                <cbc:Percent>21</cbc:Percent>
                <cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme>
            </cac:ClassifiedTaxCategory>
        </cac:Item>
        <cac:Price><cbc:PriceAmount currencyID="EUR">100.00</cbc:PriceAmount></cac:Price>
    </cac:InvoiceLine>
</Invoice>`;

const CREDIT_NOTE = `<?xml version="1.0" encoding="UTF-8"?>
<CreditNote xmlns="urn:oasis:names:specification:ubl:schema:xsd:CreditNote-2"
            xmlns:cac="urn:oasis:names:specification:ubl:schema:xsd:CommonAggregateComponents-2"
            xmlns:cbc="urn:oasis:names:specification:ubl:schema:xsd:CommonBasicComponents-2">
    <cbc:CustomizationID>urn:cen.eu:en16931:2017#compliant#urn:fdc:peppol.eu:2017:poacc:billing:3.0</cbc:CustomizationID>
    <cbc:ProfileID>urn:fdc:peppol.eu:2017:poacc:billing:01:1.0</cbc:ProfileID>
    <cbc:ID>CN-2024-001</cbc:ID>
    <cbc:IssueDate>2024-03-20</cbc:IssueDate>
    <cbc:CreditNoteTypeCode>381</cbc:CreditNoteTypeCode>
    <cbc:DocumentCurrencyCode>EUR</cbc:DocumentCurrencyCode>
    <cac:BillingReference>
        <cac:InvoiceDocumentReference>
            <cbc:ID>INV-2024-001</cbc:ID>
            <cbc:IssueDate>2024-03-15</cbc:IssueDate>
        </cac:InvoiceDocumentReference>
    </cac:BillingReference>
    <cac:AccountingSupplierParty>
        <cac:Party>
            <cbc:EndpointID schemeID="0208">0123456789</cbc:EndpointID>
            <cac:PartyName><cbc:Name>Acme Corp</cbc:Name></cac:PartyName>
            <cac:PostalAddress><cbc:CityName>Brussels</cbc:CityName><cac:Country><cbc:IdentificationCode>BE</cbc:IdentificationCode></cac:Country></cac:PostalAddress>
            <cac:PartyTaxScheme><cbc:CompanyID>BE0123456789</cbc:CompanyID><cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme></cac:PartyTaxScheme>
            <cac:PartyLegalEntity><cbc:RegistrationName>Acme Corp</cbc:RegistrationName></cac:PartyLegalEntity>
        </cac:Party>
    </cac:AccountingSupplierParty>
    <cac:AccountingCustomerParty>
        <cac:Party>
            <cbc:EndpointID schemeID="0208">9876543210</cbc:EndpointID>
            <cac:PartyName><cbc:Name>Beta NV</cbc:Name></cac:PartyName>
            <cac:PostalAddress><cbc:CityName>Antwerp</cbc:CityName><cac:Country><cbc:IdentificationCode>BE</cbc:IdentificationCode></cac:Country></cac:PostalAddress>
            <cac:PartyTaxScheme><cbc:CompanyID>BE9876543210</cbc:CompanyID><cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme></cac:PartyTaxScheme>
            <cac:PartyLegalEntity><cbc:RegistrationName>Beta NV</cbc:RegistrationName></cac:PartyLegalEntity>
        </cac:Party>
    </cac:AccountingCustomerParty>
    <cac:TaxTotal>
        <cbc:TaxAmount currencyID="EUR">21.00</cbc:TaxAmount>
        <cac:TaxSubtotal>
            <cbc:TaxableAmount currencyID="EUR">100.00</cbc:TaxableAmount>
            <cbc:TaxAmount currencyID="EUR">21.00</cbc:TaxAmount>
            <cac:TaxCategory><cbc:ID>S</cbc:ID><cbc:Percent>21</cbc:Percent><cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme></cac:TaxCategory>
        </cac:TaxSubtotal>
    </cac:TaxTotal>
    <cac:LegalMonetaryTotal>
        <cbc:LineExtensionAmount currencyID="EUR">100.00</cbc:LineExtensionAmount>
        <cbc:TaxExclusiveAmount currencyID="EUR">100.00</cbc:TaxExclusiveAmount>
        <cbc:TaxInclusiveAmount currencyID="EUR">121.00</cbc:TaxInclusiveAmount>
        <cbc:PayableAmount currencyID="EUR">121.00</cbc:PayableAmount>
    </cac:LegalMonetaryTotal>
    <cac:CreditNoteLine>
        <cbc:ID>1</cbc:ID>
        <cbc:CreditedQuantity unitCode="C62">1</cbc:CreditedQuantity>
        <cbc:LineExtensionAmount currencyID="EUR">100.00</cbc:LineExtensionAmount>
        <cac:Item>
            <cbc:Name>Refund for defective item</cbc:Name>
            <cac:ClassifiedTaxCategory><cbc:ID>S</cbc:ID><cbc:Percent>21</cbc:Percent><cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme></cac:ClassifiedTaxCategory>
        </cac:Item>
        <cac:Price><cbc:PriceAmount currencyID="EUR">100.00</cbc:PriceAmount></cac:Price>
    </cac:CreditNoteLine>
</CreditNote>`;

const SBD_WRAPPED = `<?xml version="1.0" encoding="UTF-8"?>
<StandardBusinessDocument>
    <StandardBusinessDocumentHeader>
        <DocumentIdentification><InstanceIdentifier>SBD-12345</InstanceIdentifier></DocumentIdentification>
    </StandardBusinessDocumentHeader>
    <Invoice xmlns="urn:oasis:names:specification:ubl:schema:xsd:Invoice-2"
             xmlns:cac="urn:oasis:names:specification:ubl:schema:xsd:CommonAggregateComponents-2"
             xmlns:cbc="urn:oasis:names:specification:ubl:schema:xsd:CommonBasicComponents-2">
        <cbc:CustomizationID>urn:cen.eu:en16931:2017#compliant#urn:fdc:peppol.eu:2017:poacc:billing:3.0</cbc:CustomizationID>
        <cbc:ProfileID>urn:fdc:peppol.eu:2017:poacc:billing:01:1.0</cbc:ProfileID>
        <cbc:ID>INV-SBD-001</cbc:ID>
        <cbc:IssueDate>2024-06-01</cbc:IssueDate>
        <cbc:InvoiceTypeCode>380</cbc:InvoiceTypeCode>
        <cbc:DocumentCurrencyCode>EUR</cbc:DocumentCurrencyCode>
        <cac:AccountingSupplierParty>
            <cac:Party>
                <cbc:EndpointID schemeID="0208">0123456789</cbc:EndpointID>
                <cac:PartyName><cbc:Name>Acme Corp</cbc:Name></cac:PartyName>
                <cac:PostalAddress><cbc:CityName>Brussels</cbc:CityName><cac:Country><cbc:IdentificationCode>BE</cbc:IdentificationCode></cac:Country></cac:PostalAddress>
                <cac:PartyTaxScheme><cbc:CompanyID>BE0123456789</cbc:CompanyID><cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme></cac:PartyTaxScheme>
                <cac:PartyLegalEntity><cbc:RegistrationName>Acme Corp</cbc:RegistrationName></cac:PartyLegalEntity>
            </cac:Party>
        </cac:AccountingSupplierParty>
        <cac:AccountingCustomerParty>
            <cac:Party>
                <cbc:EndpointID schemeID="0208">9876543210</cbc:EndpointID>
                <cac:PartyName><cbc:Name>Beta NV</cbc:Name></cac:PartyName>
                <cac:PostalAddress><cbc:CityName>Antwerp</cbc:CityName><cac:Country><cbc:IdentificationCode>BE</cbc:IdentificationCode></cac:Country></cac:PostalAddress>
                <cac:PartyTaxScheme><cbc:CompanyID>BE9876543210</cbc:CompanyID><cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme></cac:PartyTaxScheme>
                <cac:PartyLegalEntity><cbc:RegistrationName>Beta NV</cbc:RegistrationName></cac:PartyLegalEntity>
            </cac:Party>
        </cac:AccountingCustomerParty>
        <cac:TaxTotal>
            <cbc:TaxAmount currencyID="EUR">0.00</cbc:TaxAmount>
            <cac:TaxSubtotal>
                <cbc:TaxableAmount currencyID="EUR">50.00</cbc:TaxableAmount>
                <cbc:TaxAmount currencyID="EUR">0.00</cbc:TaxAmount>
                <cac:TaxCategory>
                    <cbc:ID>E</cbc:ID><cbc:Percent>0</cbc:Percent>
                    <cbc:TaxExemptionReasonCode>vatex-eu-132</cbc:TaxExemptionReasonCode>
                    <cbc:TaxExemptionReason>Exempt</cbc:TaxExemptionReason>
                    <cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme>
                </cac:TaxCategory>
            </cac:TaxSubtotal>
        </cac:TaxTotal>
        <cac:LegalMonetaryTotal>
            <cbc:LineExtensionAmount currencyID="EUR">50.00</cbc:LineExtensionAmount>
            <cbc:TaxExclusiveAmount currencyID="EUR">50.00</cbc:TaxExclusiveAmount>
            <cbc:TaxInclusiveAmount currencyID="EUR">50.00</cbc:TaxInclusiveAmount>
            <cbc:PayableAmount currencyID="EUR">50.00</cbc:PayableAmount>
        </cac:LegalMonetaryTotal>
        <cac:InvoiceLine>
            <cbc:ID>1</cbc:ID>
            <cbc:InvoicedQuantity unitCode="C62">1</cbc:InvoicedQuantity>
            <cbc:LineExtensionAmount currencyID="EUR">50.00</cbc:LineExtensionAmount>
            <cac:Item>
                <cbc:Name>Widget</cbc:Name>
                <cac:ClassifiedTaxCategory><cbc:ID>E</cbc:ID><cbc:Percent>0</cbc:Percent><cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme></cac:ClassifiedTaxCategory>
            </cac:Item>
            <cac:Price><cbc:PriceAmount currencyID="EUR">50.00</cbc:PriceAmount></cac:Price>
        </cac:InvoiceLine>
    </Invoice>
</StandardBusinessDocument>`;

// ─── Tests ──────────────────────────────────────────────────────────────────

describe("parseUbl — mandatory header fields", () => {
  it("extracts all mandatory header fields", () => {
    const doc = parseUbl(FULL_SPEC_INVOICE);
    expect(doc.customizationId).toBe("urn:cen.eu:en16931:2017#compliant#urn:fdc:peppol.eu:2017:poacc:billing:3.0");
    expect(doc.profileId).toBe("urn:fdc:peppol.eu:2017:poacc:billing:01:1.0");
    expect(doc.documentNumber).toBe("INV-2024-001");
    expect(doc.issueDate).toBe("2024-03-15");
    expect(doc.typeCode).toBe("380");
    expect(doc.currency).toBe("EUR");
    expect(doc.documentType).toBe("invoice");
  });
});

describe("parseUbl — optional header fields", () => {
  it("extracts all optional simple fields", () => {
    const doc = parseUbl(FULL_SPEC_INVOICE);
    expect(doc.dueDate).toBe("2024-04-15");
    expect(doc.note).toBe("Payment within 30 days");
    expect(doc.taxPointDate).toBe("2024-03-15");
    expect(doc.taxCurrencyCode).toBe("USD");
    expect(doc.accountingCost).toBe("4217:01:FA");
    expect(doc.buyerReference).toBe("PO-2024-42");
  });

  it("extracts invoice period", () => {
    const doc = parseUbl(FULL_SPEC_INVOICE);
    expect(doc.invoicePeriod).toEqual({ startDate: "2024-03-01", endDate: "2024-03-31", descriptionCode: "35" });
  });

  it("extracts all document references", () => {
    const doc = parseUbl(FULL_SPEC_INVOICE);
    expect(doc.orderReference).toBe("ORD-99");
    expect(doc.salesOrderId).toBe("SO-123");
    expect(doc.contractReference).toBe("CTR-55");
    expect(doc.projectReference).toBe("PRJ-7");
    expect(doc.despatchDocumentReference).toBe("DESP-001");
    expect(doc.receiptDocumentReference).toBe("REC-001");
    expect(doc.originatorDocumentReference).toBe("ORIG-001");
  });

  it("extracts billing references", () => {
    expect(parseUbl(FULL_SPEC_INVOICE).billingReferences).toEqual([
      { id: "INV-2023-100", issueDate: "2023-12-01" },
    ]);
  });

  it("extracts payment terms", () => {
    expect(parseUbl(FULL_SPEC_INVOICE).paymentTerms).toBe("Net 30 days, 2% discount if paid within 10 days");
  });

  it("returns undefined for absent optional fields", () => {
    const doc = parseUbl(CREDIT_NOTE);
    expect(doc.dueDate).toBeUndefined();
    expect(doc.note).toBeUndefined();
    expect(doc.taxPointDate).toBeUndefined();
    expect(doc.taxCurrencyCode).toBeUndefined();
    expect(doc.accountingCost).toBeUndefined();
    expect(doc.invoicePeriod).toBeUndefined();
    expect(doc.paymentMeans).toBeUndefined();
    expect(doc.paymentTerms).toBeUndefined();
    expect(doc.delivery).toBeUndefined();
    expect(doc.payeeParty).toBeUndefined();
    expect(doc.taxRepresentativeParty).toBeUndefined();
    expect(doc.allowanceCharges).toBeUndefined();
  });
});

describe("parseUbl — party extraction (full address spec)", () => {
  it("extracts supplier with full address fields", () => {
    const { seller } = parseUbl(FULL_SPEC_INVOICE);
    expect(seller.peppolId).toBe("0208:0123456789");
    expect(seller.name).toBe("Acme Corp");
    expect(seller.vatNumber).toBe("BE0123456789");
    expect(seller.companyId).toBe("0123456789");
    expect(seller.companyLegalForm).toBe("SA");
    expect(seller.address).toEqual({
      street: "Rue de la Loi 1",
      additionalStreet: "Building B",
      city: "Brussels",
      postalZone: "1000",
      countrySubentity: "BRU",
      addressLine: "Floor 5",
      countryCode: "BE",
    });
    expect(seller.contact).toEqual({ name: "John Doe", phone: "+32 2 123 4567", email: "john@acme.be" });
  });

  it("extracts customer with partyIdentification and companyId", () => {
    const { buyer } = parseUbl(FULL_SPEC_INVOICE);
    expect(buyer.companyId).toBe("9876543210");
    expect(buyer.contact).toEqual({ name: "Jane Smith", email: "jane@beta.be" });
  });
});

describe("parseUbl — payee party", () => {
  it("extracts payee party when present", () => {
    const { payeeParty } = parseUbl(FULL_SPEC_INVOICE);
    expect(payeeParty).toEqual({ name: "Factoring Co", companyId: "5555555555" });
  });
});

describe("parseUbl — tax representative party", () => {
  it("extracts tax representative with address and VAT", () => {
    const { taxRepresentativeParty } = parseUbl(FULL_SPEC_INVOICE);
    expect(taxRepresentativeParty).toBeDefined();
    expect(taxRepresentativeParty!.name).toBe("Tax Rep GmbH");
    expect(taxRepresentativeParty!.vatNumber).toBe("DE123456789");
    expect(taxRepresentativeParty!.address).toEqual({
      street: "Friedrichstraße 10",
      city: "Berlin",
      postalZone: "10117",
      countryCode: "DE",
    });
  });
});

describe("parseUbl — delivery", () => {
  it("extracts delivery date, location ID, address, and party name", () => {
    const { delivery } = parseUbl(FULL_SPEC_INVOICE);
    expect(delivery).toEqual({
      date: "2024-03-20",
      locationId: "5790000435944",
      address: { street: "Delivery Street 1", city: "Ghent", postalZone: "9000", countryCode: "BE" },
      partyName: "Warehouse Beta",
    });
  });
});

describe("parseUbl — payment means (all types)", () => {
  it("extracts credit transfer, card, and direct debit", () => {
    const { paymentMeans } = parseUbl(FULL_SPEC_INVOICE);
    expect(paymentMeans).toHaveLength(3);
    expect(paymentMeans![0]).toEqual({
      code: "30", paymentId: "INV-2024-001",
      iban: "BE68539007547034", accountName: "Acme Corp", bic: "BBRUBEBB",
    });
    expect(paymentMeans![1]).toEqual({
      code: "48",
      card: { accountNumber: "1234", network: "VISA", holderName: "John Doe" },
    });
    expect(paymentMeans![2]).toEqual({
      code: "49",
      mandate: { id: "MANDATE-001", payerAccount: "BE68000000001234" },
    });
  });
});

describe("parseUbl — document-level allowances/charges", () => {
  it("extracts allowances and charges with full details", () => {
    const { allowanceCharges } = parseUbl(FULL_SPEC_INVOICE);
    expect(allowanceCharges).toHaveLength(2);
    expect(allowanceCharges![0]).toEqual({
      chargeIndicator: false,
      reasonCode: "95",
      reason: "Discount",
      multiplier: 10,
      amount: 100,
      baseAmount: 1000,
      taxCategory: "S",
      taxPercent: 21,
    });
    expect(allowanceCharges![1]).toEqual({
      chargeIndicator: true,
      reason: "Freight",
      amount: 50,
      taxCategory: "S",
      taxPercent: 21,
    });
  });
});

describe("parseUbl — LegalMonetaryTotal (all fields)", () => {
  it("extracts mandatory + optional monetary amounts", () => {
    const { monetaryTotal } = parseUbl(FULL_SPEC_INVOICE);
    expect(monetaryTotal).toEqual({
      lineExtensionAmount: 1000,
      taxExclusiveAmount: 950,
      taxInclusiveAmount: 1149.5,
      allowanceTotalAmount: 100,
      chargeTotalAmount: 50,
      prepaidAmount: 200,
      payableRoundingAmount: 0.5,
      payableAmount: 950,
    });
  });
});

describe("parseUbl — TaxTotal (dual currency)", () => {
  it("extracts primary tax total with subtotals", () => {
    const { taxTotal } = parseUbl(FULL_SPEC_INVOICE);
    expect(taxTotal.taxAmount).toBe(199.5);
    expect(taxTotal.subtotals).toHaveLength(1);
    expect(taxTotal.subtotals[0]).toEqual({ taxableAmount: 950, taxAmount: 199.5, category: "S", percent: 21 });
  });

  it("extracts tax currency total", () => {
    const { taxCurrencyTotal } = parseUbl(FULL_SPEC_INVOICE);
    expect(taxCurrencyTotal).toBe(215);
  });
});

describe("parseUbl — invoice lines (full spec)", () => {
  it("extracts line with all optional fields", () => {
    const line = parseUbl(FULL_SPEC_INVOICE).lines[0];
    expect(line.id).toBe("1");
    expect(line.note).toBe("First delivery batch");
    expect(line.quantity).toBe(10);
    expect(line.unitCode).toBe("HUR");
    expect(line.lineAmount).toBe(500);
    expect(line.accountingCost).toBe("1234:56");
    expect(line.description).toBe("Consulting hours");
    expect(line.unitPrice).toBe(50);
    expect(line.taxCategory).toBe("S");
    expect(line.taxPercent).toBe(21);
  });

  it("extracts line period", () => {
    expect(parseUbl(FULL_SPEC_INVOICE).lines[0].period).toEqual({ startDate: "2024-03-01", endDate: "2024-03-15" });
  });

  it("extracts order line reference", () => {
    expect(parseUbl(FULL_SPEC_INVOICE).lines[0].orderLineReference).toBe("OL-1");
  });

  it("extracts document reference on line", () => {
    expect(parseUbl(FULL_SPEC_INVOICE).lines[0].documentReference).toEqual({ id: "DOC-REF-1", typeCode: "130" });
  });

  it("extracts line-level allowances/charges", () => {
    expect(parseUbl(FULL_SPEC_INVOICE).lines[0].allowanceCharges).toEqual([
      { chargeIndicator: false, reasonCode: "95", reason: "Line discount", amount: 25 },
    ]);
  });

  it("extracts item identifiers", () => {
    const line = parseUbl(FULL_SPEC_INVOICE).lines[0];
    expect(line.itemDescription).toBe("Expert consulting services for Q1");
    expect(line.buyersItemId).toBe("BUY-001");
    expect(line.sellersItemId).toBe("SEL-001");
    expect(line.standardItemId).toBe("1234567890128");
    expect(line.originCountry).toBe("BE");
  });

  it("extracts commodity classifications", () => {
    expect(parseUbl(FULL_SPEC_INVOICE).lines[0].commodityClassifications).toEqual([
      { code: "72212000", listId: "STI" },
    ]);
  });

  it("extracts additional item properties", () => {
    expect(parseUbl(FULL_SPEC_INVOICE).lines[0].additionalProperties).toEqual([
      { name: "Color", value: "Blue" },
    ]);
  });

  it("extracts price base quantity and price allowance", () => {
    const line = parseUbl(FULL_SPEC_INVOICE).lines[0];
    expect(line.baseQuantity).toBe(1);
    expect(line.priceAllowance).toEqual({ amount: 5, baseAmount: 55 });
  });

  it("extracts sellers item ID on minimal line", () => {
    expect(parseUbl(FULL_SPEC_INVOICE).lines[1].sellersItemId).toBe("SEL-002");
  });

  it("returns undefined for absent optional line fields", () => {
    const line = parseUbl(FULL_SPEC_INVOICE).lines[1];
    expect(line.note).toBeUndefined();
    expect(line.accountingCost).toBeUndefined();
    expect(line.period).toBeUndefined();
    expect(line.orderLineReference).toBeUndefined();
    expect(line.allowanceCharges).toBeUndefined();
    expect(line.buyersItemId).toBeUndefined();
    expect(line.standardItemId).toBeUndefined();
    expect(line.additionalProperties).toBeUndefined();
    expect(line.baseQuantity).toBeUndefined();
    expect(line.priceAllowance).toBeUndefined();
  });
});

describe("parseUbl — credit notes", () => {
  it("detects credit note type and extracts CreditNoteLine", () => {
    const doc = parseUbl(CREDIT_NOTE);
    expect(doc.documentType).toBe("credit_note");
    expect(doc.typeCode).toBe("381");
    expect(doc.lines).toHaveLength(1);
    expect(doc.lines[0].description).toBe("Refund for defective item");
  });

  it("extracts billing reference", () => {
    expect(parseUbl(CREDIT_NOTE).billingReferences).toEqual([{ id: "INV-2024-001", issueDate: "2024-03-15" }]);
  });
});

describe("parseUbl — SBD wrapper", () => {
  it("unwraps SBD and preserves instanceIdentifier", () => {
    const doc = parseUbl(SBD_WRAPPED);
    expect(doc.instanceIdentifier).toBe("SBD-12345");
    expect(doc.documentNumber).toBe("INV-SBD-001");
    expect(doc.lines).toHaveLength(1);
  });

  it("extracts tax exemption details", () => {
    const { taxTotal } = parseUbl(SBD_WRAPPED);
    expect(taxTotal.subtotals[0]).toEqual({
      taxableAmount: 50, taxAmount: 0, category: "E", percent: 0,
      exemptionReasonCode: "vatex-eu-132", exemptionReason: "Exempt",
    });
  });
});

describe("parseUbl — validation", () => {
  const cases = [
    { name: "missing document ID", xml: '<Invoice xmlns="urn:oasis:names:specification:ubl:schema:xsd:Invoice-2"><cbc:IssueDate xmlns:cbc="urn:oasis:names:specification:ubl:schema:xsd:CommonBasicComponents-2">2024-01-01</cbc:IssueDate></Invoice>', error: /document ID/i },
    { name: "missing issue date", xml: '<Invoice xmlns="urn:oasis:names:specification:ubl:schema:xsd:Invoice-2"><cbc:ID xmlns:cbc="urn:oasis:names:specification:ubl:schema:xsd:CommonBasicComponents-2">X</cbc:ID></Invoice>', error: /issue date/i },
    { name: "unsupported document type", xml: '<Order xmlns="urn:oasis:names:specification:ubl:schema:xsd:Order-2"><cbc:ID>1</cbc:ID></Order>', error: /unsupported/i },
  ];
  for (const { name, xml, error } of cases) {
    it(`throws on ${name}`, () => { expect(() => parseUbl(xml)).toThrow(error); });
  }
});

describe("parseUbl — attachments", () => {
  it("extracts embedded attachments", () => {
    const { attachments } = parseUbl(FULL_SPEC_INVOICE);
    expect(attachments).toHaveLength(1);
    expect(attachments[0]).toEqual({
      id: "ATT-1", description: "Timesheet",
      mimeCode: "application/pdf", filename: "timesheet.pdf", base64Content: "dGVzdA==",
    });
  });
});
