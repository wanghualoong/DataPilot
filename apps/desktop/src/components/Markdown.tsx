import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { openExternal } from "./links";

// Model text is untrusted: no raw HTML, links open in the system browser.
export function Markdown({ text }: { text: string }) {
  return (
    <div className="md">
      <ReactMarkdown remarkPlugins={[remarkGfm]} skipHtml
        components={{ a: ({ href, children }) => <a href={href} target="_blank" rel="noreferrer noopener"
          onClick={(e) => { if (href && /^https?:/i.test(href)) { e.preventDefault(); openExternal(href); } }}>{children}</a> }}>
        {text}
      </ReactMarkdown>
    </div>
  );
}
