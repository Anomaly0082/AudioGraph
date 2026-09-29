import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";

export default function AssistantMarkdown({ text }: { text: string }) {
  return (
    <div className="assistant-markdown">
      <Markdown
        remarkPlugins={[remarkGfm]}
        components={{
          // Model output is untrusted. Do not enable raw HTML or automatically load images.
          img: ({ alt }) => (
            <span className="markdown-image-note">
              {alt ? `[图片：${alt}]` : "[图片]"}
            </span>
          ),
          a: ({ href, children }) => {
            if (!href || !/^https?:\/\//i.test(href))
              return <span>{children}</span>;
            return (
              <a href={href} target="_blank" rel="noopener noreferrer">
                {children}
              </a>
            );
          },
          table: ({ children }) => (
            <div className="markdown-table">
              <table>{children}</table>
            </div>
          ),
        }}
      >
        {text}
      </Markdown>
    </div>
  );
}
