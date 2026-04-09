import { createHmac, timingSafeEqual } from "crypto";
import { parseUbl, type ParsedUBLDocument } from "./parseUbl";

const USER_AGENT = "RootCX/2.0";

const STATUS_MAP: Record<string, string> = {
  SENT: "sent", DELIVERED: "delivered", FAILED: "failed",
  ACCEPTED: "accepted", REJECTED: "rejected",
};

function verifySignature(secret: string, rawBody: string, signature: string): boolean {
  const expected = createHmac("sha256", secret).update(rawBody).digest("hex");
  if (expected.length !== signature.length) return false;
  return timingSafeEqual(Buffer.from(expected), Buffer.from(signature));
}

export interface WebhookDeps {
  collectionOp: (op: string, entity: string, data: Record<string, unknown>) => Promise<any>;
  uploadFile: (content: string | Uint8Array, filename: string, contentType: string) => Promise<string>;
  dokapiRequest: (config: any, method: string, endpoint: string, body?: unknown) => Promise<any>;
}

export async function handleWebhook(params: any, deps: WebhookDeps) {
  const { body, config, rawBody, headers } = params;

  if (config?.webhookSecret) {
    const signature = headers?.["x-signature"];
    if (!signature || !rawBody) throw new Error("missing webhook signature");
    const raw = Buffer.from(rawBody, "base64").toString("utf-8");
    if (!verifySignature(config.webhookSecret, raw, signature)) throw new Error("invalid webhook signature");
  }

  if (!body?.event || !body?.body) return { skipped: true, reason: "invalid payload" };

  const { event: eventType, body: data } = body;

  switch (eventType) {
    case "outgoing-peppol-documents.sent":
    case "outgoing-peppol-documents.delivered":
    case "outgoing-peppol-documents.failed": {
      const { ulid, status, as4MessageId, statusMessage } = data;
      const mapped = STATUS_MAP[status] || status?.toLowerCase();
      try {
        await deps.collectionOp("insert", "outgoing_status", {
          document_ulid: ulid,
          status: mapped,
          as4_message_id: as4MessageId || "",
          error_message: statusMessage || "",
          delivered_at: status === "DELIVERED" ? new Date().toISOString() : "",
        });
      } catch { /* best effort */ }
      return { event: "document_status", dokapiUlid: ulid, status: mapped };
    }

    case "participant-registrations.active":
      return { event: "participant_status", dokapiUlid: data.ulid, status: "active" };

    case "participant-registrations.failed":
      return { event: "participant_status", dokapiUlid: data.ulid, status: "failed", errorMessage: data.errorMessage };

    case "incoming-peppol-documents.received": {
      const { ulid: documentUlid, presignedUrl, sender, receiver, instanceIdentifier, as4MessageId } = data;
      if (!documentUlid || !presignedUrl) return { event: "incoming_document", error: "missing ulid or presignedUrl" };

      let xml: string | undefined;
      let parsed: ParsedUBLDocument | undefined;
      try {
        const res = await fetch(presignedUrl, { headers: { "User-Agent": USER_AGENT } });
        if (!res.ok) throw new Error(`Download failed: ${res.status}`);
        xml = await res.text();
        parsed = parseUbl(xml);
      } catch (err: any) {
        return {
          event: "incoming_document",
          documentUlid, senderPeppolId: sender?.value, receiverPeppolId: receiver?.value,
          instanceIdentifier, as4MessageId,
          status: "download_failed", error: err.message,
        };
      }

      try {
        const xmlFileId = await deps.uploadFile(xml, `${documentUlid}.xml`, "application/xml");

        const uploadedAttachments = await Promise.all((parsed.attachments ?? []).map(async (att) => {
          const fileId = await deps.uploadFile(
            Uint8Array.from(atob(att.base64Content), c => c.charCodeAt(0)),
            att.filename,
            att.mimeCode || "application/octet-stream",
          );
          return { id: att.id, description: att.description, mimeCode: att.mimeCode, filename: att.filename, fileId };
        }));

        await deps.collectionOp("insert", "incoming_documents", {
          document_ulid: documentUlid,
          document_type: parsed.documentType,
          document_number: parsed.documentNumber,
          issue_date: parsed.issueDate,
          due_date: parsed.dueDate || "",
          currency: parsed.currency,
          amount: parsed.monetaryTotal.payableAmount,
          sender_peppol_id: parsed.seller.peppolId,
          sender_name: parsed.seller.name,
          sender_vat: parsed.seller.vatNumber || "",
          receiver_peppol_id: parsed.buyer.peppolId,
          receiver_name: parsed.buyer.name,
          status: "received",
          instance_identifier: instanceIdentifier || "",
          as4_message_id: as4MessageId || "",
          xml: xmlFileId,
          attachments: uploadedAttachments,
        });
      } catch (err: any) {
        return {
          event: "incoming_document", documentUlid,
          status: "persist_failed", error: err.message,
        };
      }

      try { await deps.dokapiRequest(config, "POST", `/incoming-peppol-documents/${documentUlid}/confirm`, {}); } catch {}

      return { event: "incoming_document", documentUlid, status: "received" };
    }

    default:
      return { skipped: true, reason: `unhandled event: ${eventType}` };
  }
}
