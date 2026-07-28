import Link from '@docusaurus/Link';
import useBaseUrl from '@docusaurus/useBaseUrl';
import useDocusaurusContext from '@docusaurus/useDocusaurusContext';
import Layout from '@theme/Layout';

const CARDS = [
  {
    title: 'Calls',
    body: 'Place and answer, hold and resume, blind and attended transfer, bridge two calls or conference several — with session timers so a vanished far end ends the call.',
    to: '/docs/guides/place-a-call',
  },
  {
    title: 'Audio',
    body: 'G.711 both ways and Opus behind a feature. DTMF, WAV playback and recording, an adaptive jitter buffer, and mid-call quality statistics.',
    to: '/docs/guides/answer-a-call',
  },
  {
    title: 'Security that cannot be turned off',
    body: 'TLS and secure WebSocket with certificate verification that has no off switch, and SRTP negotiated automatically when the signalling protects the key.',
    to: '/docs/guides/does-this-fit',
  },
  {
    title: 'A library first',
    body: 'The protocol core does no I/O: parser, transactions and dialogs are pure state machines you can take without a socket layer attached.',
    to: '/docs/guides/as-a-library',
  },
  {
    title: 'A phone you can script',
    body: 'sipx dial, answer and register from a shell — every command speaks --json and returns a distinct exit code per outcome.',
    to: '/docs/reference/cli',
  },
  {
    title: 'The SDK (preview)',
    body: 'Call events out, instructions in: the contract that will let you build call behaviour without writing Rust. Specified, experimental, in progress.',
    to: '/docs/sdk/overview',
  },
];

export default function Home() {
  const { siteConfig } = useDocusaurusContext();
  return (
    <Layout description={siteConfig.tagline}>
      <header className="hero--sipx">
        <img src={useBaseUrl('/img/logo.svg')} alt="" />
        <h1 className="hero__title">sipx</h1>
        <p className="hero__subtitle">{siteConfig.tagline} As a library you embed, or as a command you run.</p>
        <div>
          <Link className="button button--primary button--lg" to="/docs/getting-started">
            Get started
          </Link>{' '}
          <Link className="button button--secondary button--lg" to="/docs/guides/does-this-fit">
            Does sipx fit?
          </Link>
        </div>
      </header>
      <main>
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
