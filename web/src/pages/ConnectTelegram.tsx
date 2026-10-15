import { useState } from "react";
import {
  ArrowLeft,
  ExternalLink,
  KeyRound,
  Loader2,
  MessageSquare,
  Phone,
  ShieldAlert,
  Smartphone,
} from "lucide-react";
import { api, ApiError } from "../api";
import { Alert, Button, Card, Field, PasswordField } from "../ui";

type Step = "guide" | "credentials" | "phone" | "code" | "password" | "done";

export function ConnectTelegram({ onConnected }: { onConnected: () => void }) {
  const [step, setStep] = useState<Step>("guide");
  const [apiId, setApiId] = useState("");
  const [apiHash, setApiHash] = useState("");
  const [phone, setPhone] = useState("");
  const [code, setCode] = useState("");
  const [twoFactor, setTwoFactor] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [hint, setHint] = useState("");

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

  const sendCode = () =>
    run(async () => {
      const result = await api.telegram.start(Number(apiId), apiHash, phone);
      setHint(result.phone_hint);
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

        {step === "credentials" && (
          <form
            className="space-y-4"
            onSubmit={(e) => {
              e.preventDefault();
              setError(null);
              setStep("phone");
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
              subtitle={`Telegram sent a code to ${hint}. It arrives in the Telegram app itself, not by SMS, if you are signed in elsewhere.`}
            />
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
                Start over
              </Button>
              <Button type="submit" busy={busy} className="flex-1">
                Verify
              </Button>
            </div>
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
