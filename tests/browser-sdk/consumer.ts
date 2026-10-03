import { SipxClient, SipxMediaError, experimental } from "@sipx/browser";
export async function start(transport: {
  scheme: "wss";
  host: string;
  resource: string;
}) {
  const client = await SipxClient.create({
    aor: "sip:browser@localhost",
    auth: { username: "browser", password: "fixture-password" },
    transport,
  });
  client.on("incoming", (call) => {
    void call.answer().catch((error) => {
      if (error instanceof SipxMediaError) void call.hangup();
    });
  });
  await client.register();
  const call = await client.dial("sip:native@localhost");
  call.mute(false);
  await call.hangup();
  await client.close();
  return experimental;
}
