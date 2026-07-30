import Link from '@docusaurus/Link';
import useBaseUrl from '@docusaurus/useBaseUrl';
import useDocusaurusContext from '@docusaurus/useDocusaurusContext';
import Layout from '@theme/Layout';

const CARDS = [
  {
    title: 'A scriptable CLI',
    body: 'Place, answer, and register from a shell with JSON results and distinct exit codes. Playback and recording use WAV files, not a microphone or speaker.',
    to: '/docs/reference/cli',
  },
  {
    title: 'A Rust call framework',
    body: 'Place and answer calls, register, transfer, play and record audio, receive DTMF, and inspect call quality from typed Rust APIs.',
    to: '/docs/guides/as-a-library',
  },
  {
    title: 'Telephony audio',
    body: 'G.711 in both directions, WAV playback and recording, DTMF, jitter buffering, and quality statistics. Opus is selectable through an optional library feature.',
    to: '/docs/guides/answer-a-call',
  },
  {
    title: 'A Sans-I/O core',
    body: 'The protocol core does no I/O: parser, transactions and dialogs are pure state machines you can take without a socket layer attached.',
    to: '/docs/guides/as-a-library',
  },
  {
    title: 'Secure library transports',
    body: 'The Rust transport layer supports TLS and secure WebSocket with certificate verification, and calls can negotiate SRTP when signalling protects the key.',
    to: '/docs/reference/security',
  },
  {
    title: 'Measured RFC coverage',
    body: 'A generated registry distinguishes implemented behaviour, partial support, syntax-only parsing, and work that has not started.',
    to: '/docs/reference/compliance',
  },
];

export default function Home() {
  const { siteConfig } = useDocusaurusContext();
  return (
    <Layout description={siteConfig.tagline}>
      <header className="hero--sipx">
        <img src={useBaseUrl('/img/logo.svg')} alt="" />
        <h1 className="hero__title">sipx</h1>
        <p className="hero__subtitle">
          {siteConfig.tagline} Build a programmable SIP endpoint as a Rust library you embed or
          a command you run. sipx is not a proxy, registrar, or configuration-driven PBX.
        </p>
        <div>
          <Link className="button button--primary button--lg" to="/docs/getting-started">
            Try the CLI
          </Link>{' '}
          <Link className="button button--secondary button--lg" to="/docs/guides/as-a-library">
            Use the Rust libraries
          </Link>
        </div>
      </header>
      <main>
        <aside className="sipx-development-notice" aria-label="Documentation version">
          <strong>Development documentation:</strong> this site describes <code>main</code>, which
          can move ahead of the latest tagged release. The getting-started guide provides both
          reproducible tagged and development install commands.
        </aside>
        <div className="sipx-cards">
          {CARDS.map((card) => (
            <Link key={card.title} className="sipx-card" to={card.to}>
              <h3>{card.title}</h3>
              <p>{card.body}</p>
            </Link>
          ))}
        </div>
      </main>
    </Layout>
  );
}
