import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { t } from "../i18n";
import type { CitationView, Segment } from "../lib/api";

const OUTSIDE_MARKER = /\[\[spoza-dokumentu\]\]/g;

/** Skleja bloki odpowiedzi w jeden Markdown, wstawiając znaczniki Cytatów jako linki #cite-N. */
function toMarkdown(segments: Segment[]): { md: string; citations: CitationView[] } {
  const citations: CitationView[] = [];
  let md = "";
  for (const seg of segments) {
    md += seg.text.replace(OUTSIDE_MARKER, `[${t("chat.outside")}](#outside)`);
    const seen = new Set<string>();
    for (const c of seg.citations) {
      const key = `${c.page}`;
      if (seen.has(key)) continue;
      seen.add(key);
      citations.push(c);
      const label = c.page ? t("chat.page", { page: c.page }) : "↗";
      md += ` [${label}](#cite-${citations.length - 1})`;
    }
  }
  return { md, citations };
}

export function MessageBody({ segments, onCitation }: { segments: Segment[]; onCitation(c: CitationView): void }) {
  const { md, citations } = toMarkdown(segments);
  return (
    <div className="markdown">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          a({ href, children }) {
            if (href === "#outside") return <span className="outside-badge">{children}</span>;
            if (href?.startsWith("#cite-")) {
              const c = citations[Number(href.slice(6))];
              return (
                <button className="cite" title={c?.citedText} onClick={() => c && onCitation(c)}>
                  {children}
                </button>
              );
            }
            return (
              <a href={href} target="_blank" rel="noreferrer">
                {children}
              </a>
            );
          },
        }}
      >
        {md}
      </ReactMarkdown>
    </div>
  );
}

export function plainText(segments: Segment[]): string {
  return segments.map((s) => s.text).join("").replace(OUTSIDE_MARKER, `${t("chat.outside")}:`);
}
