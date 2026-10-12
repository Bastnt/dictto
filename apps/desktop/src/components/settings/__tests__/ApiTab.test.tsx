import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ApiTab } from "../TranscriptionTab";

describe("API endpoint settings", () => {
  it("disables cleanup without erasing the chosen model", async () => {
    const onSave = vi.fn();
    render(<ApiTab hasApiKey={true} apiKeyHint="hidden" onSaveApiKey={vi.fn()}
      onRemoveApiKey={vi.fn()} settings={{ cleanup_enabled: "false", llm_model: "cleanup-model" }} onSave={onSave} />);
    expect(screen.getByLabelText("Cleanup model")).toBeDisabled();
    await userEvent.click(screen.getByRole("switch", {name: "Text cleanup"}));
    expect(onSave).toHaveBeenCalledWith("cleanup_enabled", "true");
  });
  it("shows and saves a custom endpoint without requiring a key first", async () => {
    const onSave = vi.fn();
    const user = userEvent.setup();
    render(<ApiTab hasApiKey={false} apiKeyHint="" onSaveApiKey={vi.fn()}
      onRemoveApiKey={vi.fn()} settings={{ api_base_url: "http://127.0.0.1:8000/v1",
        transcription_model: "whisper-1", llm_model: "cleanup-model" }} onSave={onSave} />);
    const input = screen.getByLabelText("API base URL");
    expect(input).toHaveValue("http://127.0.0.1:8000/v1");
    expect(screen.getByLabelText("Transcription model")).toHaveValue("whisper-1");
    expect(screen.getByLabelText("Cleanup model")).toHaveValue("cleanup-model");
    await user.clear(input);
    await user.type(input, "http://127.0.0.1:9000/v1");
    await user.tab();
    expect(onSave).toHaveBeenCalledWith("api_base_url", "http://127.0.0.1:9000/v1");
  });
});
