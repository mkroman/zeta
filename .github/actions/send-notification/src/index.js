import * as core from "@actions/core";

async function main() {
  const channel = core.getInput("channel", { required: true });
  const message = core.getInput("message", { required: true });
  const network = core.getInput("network", { required: true });
  const webhookUrl = core.getInput("webohok-url", { required: true });
  const token = core.getInput("token", { required: true });

  core.setSecret(token);

  const headers = {
    "authorization": `Bearer ${token}`,
    "content-type": "application/json"
  };
  const body = {
    "method": "message",
    "params": {
      network, channel, message
    }
  };

  return await fetch(webhookUrl, {
    method: "POST", headers, body
  }).then((response) => {
    if (!response.ok) {
      core.debug(`Webhook response body: ${response.text()}`);
      throw new Error(`Webhook returned unexpected HTTP error: ${response.status}`);
    }

    return response.json();
  });
}

try {
  await main();
} catch (error) {
  core.setFailed(error.message);
}

