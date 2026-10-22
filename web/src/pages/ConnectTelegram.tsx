import { useEffect, useState } from "react";
import {
  ArrowLeft,
  ExternalLink,
  KeyRound,
  Loader2,
  MessageSquare,
  Phone,
  ShieldAlert,
  QrCode,
  Smartphone,
} from "lucide-react";
import { api, ApiError } from "../api";
import { QRCodeSVG } from "qrcode.react";
import { Alert, Button, Card, Field, PasswordField } from "../ui";

type Step = "guide" | "credentials" | "method" | "qr" | "phone" | "code" | "password" | "done";

export function ConnectTelegram({ onConnected }: { onConnected: () => void }) {
  const [step, setStep] = useState<Step>("guide");
  const [apiId, setApiId] = useState("");
  const [apiHash, setApiHash] = useState("");
  const [phone, setPhone] = useState("");
  const [code, setCode] = useState("");
  const [twoFactor, setTwoFactor] = useState("");
  const [busy, setBusy] = useState(false);
  const [qrUrl, setQrUrl] = useState<string | null>(null);
  const [delivery, setDelivery] = useState<string>("");
  const [error, setError] = useState<string | null>(null);

  const fail = (caught: unknown) =>
    setError(
      caught instanceof ApiError
        ? caught.message
        : "Something went wrong. Please try again.",
    );

  async function run(action: () => Promise<void>) {
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (caught) {
      fail(caught);
    } finally {
      setBusy(false);
    }
  }

  /// Requests a QR token and moves to the QR screen.
  const startQr = () =>
    run(async () => {
      setQrUrl(null);
      setStep("qr");
      const result = await api.telegram.qrStart(Number(apiId), apiHash);
      setQrUrl(result.url);
    });

  /// Polls while the QR screen is up.
  ///
  /// Telegram's token lasts about a minute, so this also refreshes it when
  /// the poll comes back with a new one — otherwise the code on screen
  /// goes stale and scanning it does nothing, with no visible reason.
  useEffect(() => {
    if (step !== "qr") return;

    let cancelled = false;
    const tick = async () => {
      try {
        const result = await api.telegram.qrPoll();
        if (cancelled) return;
        if (result.connected) {
          setStep("done");
          onConnected();
          return;
        }
        if (result.url) setQrUrl(result.url);
      } catch (caught) {
        if (cancelled) return;
        fail(caught);
        setStep("method");
      }
    };

    const timer = window.setInterval(tick, 2500);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [step, onConnected]);

  const sendCode = () =>
    run(async () => {
      const sent = await api.telegram.start(Number(apiId), apiHash, phone);
      setDelivery(sent.delivery);
      setStep("code");
    });

  const submitCode = () =>
    run(async () => {
      const result = await api.telegram.code(code);
      if (result.needs_password) {
        setStep("password");
      } else {
        setStep("done");
        onConnected();
      }
    });

  const submitPassword = () =>
    run(async () => {
      await api.telegram.password(twoFactor);
      setStep("done");
      onConnected();
    });

  return (
    <div className="mx-auto max-w-xl px-4 py-10">
      <Progress step={step} />

      <Card className="mt-5 p-6">
        {error && (
          <div className="mb-4">
            <Alert tone="error">{error}</Alert>
          </div>
        )}

        {step === "guide" && <Guide onNext={() => setStep("credentials")} />}

        {step === "method" && (
          <div>
            <Header
              icon={Smartphone}
              title="How would you like to sign in?"
              subtitle="Both link the same account. The QR code is usually quicker and avoids waiting for a text."
            />
            <div className="space-y-2">
              <MethodCard
                icon={QrCode}
                title="Scan a QR code"
                detail="Open Telegram on your phone → Settings → Devices → Link Desktop Device. Nothing to type."
                recommended
                onClick={() => void startQr()}
              />
              <MethodCard
                icon={Phone}
                title="Use my phone number"
                detail="Telegram sends a code to your account. Needs the country code."
                onClick={() => setStep("phone")}
              />
            </div>
            <Button
              variant="ghost"
              onClick={() => setStep("credentials")}
              className="mt-4 w-full"
            >
              <ArrowLeft className="h-3.5 w-3.5" />
              Back
            </Button>
          </div>
        )}

        {step === "qr" && (
          <div className="text-center">
            <Header
              icon={QrCode}
              title="Scan this with Telegram"
              subtitle="On your phone: Settings → Devices → Link Desktop Device, then point the camera here."
            />
            {qrUrl ? (
              <div className="mx-auto mb-4 w-fit rounded-[var(--radius-card)] bg-white p-4">
                <QRCodeSVG value={qrUrl} size={200} level="M" />
              </div>
            ) : (
              <div className="py-12">
                <Loader2 className="mx-auto h-5 w-5 animate-spin text-ink-faint" />
              </div>
            )}
            <p className="mb-4 text-xs text-ink-muted">
              Waiting for the scan… this page updates by itself.
            </p>
            <div className="flex gap-2">
              <Button
                variant="ghost"
                onClick={() => setStep("method")}
                className="flex-1"
              >
                <ArrowLeft className="h-3.5 w-3.5" />
                Back
              </Button>
              <Button variant="ghost" onClick={() => void startQr()} className="flex-1">
                New code
              </Button>
            </div>
          </div>
        )}

        {step === "credentials" && (
          <form
            className="space-y-4"
            onSubmit={(e) => {
              e.preventDefault();
              setError(null);
              setStep("method");
            }}
          >
            <Header
              icon={KeyRound}
              title="Your Telegram API credentials"
              subtitle="From my.telegram.org — these stay tied to your own account."
            />
            <Field
              label="API ID"
              inputMode="numeric"
              required
              value={apiId}
              onChange={(e) => setApiId(e.target.value)}
              placeholder="1234567"
              hint="A number, usually 7–8 digits."
            />
            <Field
              label="API Hash"
              required
              value={apiHash}
              onChange={(e) => setApiHash(e.target.value)}
              placeholder="0123456789abcdef0123456789abcdef"
              hint="32 characters of hex."
            />
            <div className="flex gap-2">
              <Button variant="ghost" type="button" onClick={() => setStep("guide")}>
                <ArrowLeft className="h-3.5 w-3.5" />
                Back
              </Button>
              <Button type="submit" className="flex-1">
                Continue
              </Button>
            </div>
          </form>
        )}

        {step === "phone" && (
          <form
            className="space-y-4"
            onSubmit={(e) => {
              e.preventDefault();
              void sendCode();
            }}
          >
            <Header
              icon={Phone}
              title="Your phone number"
              subtitle="The number on your Telegram account, with its country code."
            />
            <Field
              label="Phone number"
              type="tel"
              required
              value={phone}
              onChange={(e) => setPhone(e.target.value)}
              placeholder="+91 98765 43210"
              hint="Telegram will send a code to this number."
            />
            <div className="flex gap-2">
              <Button
                variant="ghost"
                type="button"
                onClick={() => setStep("credentials")}
              >
                <ArrowLeft className="h-3.5 w-3.5" />
                Back
              </Button>
              <Button type="submit" busy={busy} className="flex-1">
                Send me a code
              </Button>
            </div>
          </form>
        )}

        {step === "code" && (
          <form
            className="space-y-4"
            onSubmit={(e) => {
              e.preventDefault();
              void submitCode();
            }}
          >
            <Header
              icon={MessageSquare}
              title="Enter the code"
              // The full number, not the masked hint: a mistyped digit is
              // by far the most common reason no code arrives, and a
              // masked version hides exactly the mistake worth spotting.
              subtitle={delivery || `Telegram sent a code to ${phone}.`}
            />

            <div className="rounded-[var(--radius-control)] border border-border bg-surface p-3">
              <p className="text-xs leading-relaxed text-ink-muted">
                <strong className="text-ink">Where to look.</strong> If you are
                signed in to Telegram anywhere else — phone, desktop, web — the
                code arrives there as a message from{" "}
                <strong className="text-ink">Telegram</strong> (the official
                account with a blue tick), not as an SMS. Check that chat first.
              </p>
            </div>

            <Field
              label="Login code"
              inputMode="numeric"
              autoFocus
              required
              value={code}
              onChange={(e) => setCode(e.target.value)}
              placeholder="12345"
            />

            <div className="flex gap-2">
              <Button variant="ghost" type="button" onClick={() => setStep("phone")}>
                <ArrowLeft className="h-3.5 w-3.5" />
                Change number
              </Button>
              <Button type="submit" busy={busy} className="flex-1">
                Verify
              </Button>
            </div>

            <button
              type="button"
              onClick={() => void sendCode()}
              disabled={busy}
              className="w-full text-center text-xs text-accent hover:underline disabled:opacity-50"
            >
              Did not arrive? Send another code
            </button>
          </form>
        )}

        {step === "password" && (
          <form
            className="space-y-4"
            onSubmit={(e) => {
              e.preventDefault();
              void submitPassword();
            }}
          >
            <Header
              icon={ShieldAlert}
              title="Two-step verification"
              subtitle="Your account has a cloud password. Enter it to finish."
            />
            <PasswordField
              label="Two-step password"
              autoFocus
              required
              value={twoFactor}
              onChange={(e) => setTwoFactor(e.target.value)}
              hint="This is your Telegram cloud password, not the code."
            />
            <Button type="submit" busy={busy} className="w-full">
              Finish connecting
            </Button>
          </form>
        )}

        {step === "done" && (
          <div className="py-6 text-center">
            <p className="text-base font-semibold text-ink">
              Telegram connected
            </p>
            <p className="mt-2 text-sm text-ink-muted">
              Your drive is linked to your own Telegram account.
            </p>
          </div>
        )}
      </Card>
    </div>
  );
}

function Header({
  icon: Icon,
  title,
  subtitle,
}: {
  icon: typeof Phone;
  title: string;
  subtitle: string;
}) {
  return (
    <div className="mb-5 flex gap-3">
      <div className="mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-[var(--radius-control)] bg-accent-soft">
        <Icon className="h-4 w-4 text-accent" />
      </div>
      <div>
        <h2 className="text-sm font-semibold text-ink">{title}</h2>
        <p className="mt-0.5 text-xs leading-relaxed text-ink-muted">{subtitle}</p>
      </div>
    </div>
  );
}

function MethodCard({
  icon: Icon,
  title,
  detail,
  recommended = false,
  onClick,
}: {
  icon: typeof Phone;
  title: string;
  detail: string;
  recommended?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className="flex w-full gap-3 rounded-[var(--radius-control)] border border-border
                 p-3 text-left transition-colors hover:border-accent/50 hover:bg-surface"
    >
      <div className="mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-[var(--radius-control)] bg-accent-soft">
        <Icon className="h-4 w-4 text-accent" />
      </div>
      <div className="min-w-0">
        <p className="flex items-center gap-2 text-sm font-medium text-ink">
          {title}
          {recommended && (
            <span className="rounded-full bg-success-soft px-1.5 py-0.5 text-[10px] font-medium text-[var(--color-success)]">
              Easier
            </span>
          )}
        </p>
        <p className="mt-0.5 text-xs leading-relaxed text-ink-muted">{detail}</p>
      </div>
    </button>
  );
}

function Guide({ onNext }: { onNext: () => void }) {
  const steps = [
    {
      title: "Open Telegram's developer portal",
      body: (
        <>
          Go to{" "}
          <a
            href="https://my.telegram.org"
            target="_blank"
            rel="noreferrer"
            className="inline-flex items-center gap-1 text-accent hover:underline"
          >
            my.telegram.org
            <ExternalLink className="h-3 w-3" />
          </a>{" "}
          and sign in with your phone number. The confirmation code arrives in
          the Telegram app.
        </>
      ),
    },
    {
      title: "Open API development tools",
      body: <>Click <strong>API development tools</strong> and create an application. Any name and description will do — nobody else sees them.</>,
    },
    {
      title: "Copy both values",
      body: (
        <>
          You will see an <strong>App api_id</strong> (a number) and an{" "}
          <strong>App api_hash</strong> (32 characters). You need both on the
          next screen.
        </>
      ),
    },
  ];

  return (
    <div>
      <Header
        icon={Smartphone}
        title="Connect your Telegram account"
        subtitle="Your files are stored in your own Telegram. This takes about two minutes, once."
      />

      <ol className="space-y-4">
        {steps.map((item, index) => (
          <li key={item.title} className="flex gap-3">
            <span className="mt-0.5 flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-accent text-[11px] font-semibold text-white">
              {index + 1}
            </span>
            <div>
              <p className="text-sm font-medium text-ink">{item.title}</p>
              <p className="mt-0.5 text-xs leading-relaxed text-ink-muted">
                {item.body}
              </p>
            </div>
          </li>
        ))}
      </ol>

      {/* The desktop app tells users their credentials never leave the
          device. That is true there and false here — this is a server, and
          saying otherwise would be a lie people make security decisions on. */}
      <div className="mt-5 rounded-[var(--radius-control)] border border-border bg-surface p-3">
        <p className="text-xs leading-relaxed text-ink-muted">
          <strong className="text-ink">What this server stores.</strong> To act
          on your Telegram, this server keeps a login session for your account
          — the same access as being signed in. It is encrypted with a key
          derived from your password and held only while you are signed in, so
          a stolen copy of the database cannot be used. You can disconnect at
          any time and it is deleted.
        </p>
      </div>

      <Button onClick={onNext} className="mt-5 w-full">
        I have my API ID and hash
      </Button>
    </div>
  );
}

function Progress({ step }: { step: Step }) {
  const order: Step[] = ["guide", "credentials", "phone", "code", "done"];
  // The 2FA screen is conditional, so it shares a position with the code
  // step rather than making the bar jump for the people who have it.
  const current = step === "password" ? 3 : order.indexOf(step);

  return (
    <div className="flex items-center gap-1.5">
      {order.map((name, index) => (
        <div
          key={name}
          className={`h-1 flex-1 rounded-full transition-colors ${
            index <= current ? "bg-accent" : "bg-border"
          }`}
        />
      ))}
    </div>
  );
}

export function Connecting() {
  return (
    <div className="flex items-center justify-center py-20 text-ink-faint">
      <Loader2 className="h-5 w-5 animate-spin" />
    </div>
  );
}
