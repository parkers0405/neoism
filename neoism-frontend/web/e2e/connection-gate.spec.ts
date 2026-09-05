import { test, expect } from "@playwright/test";

test.describe("web connection/workspace gate", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await expect(page.locator(".connection-form")).toBeVisible();
  });

  test("floats on the page while controls use the elevated theme surface", async ({
    page,
  }) => {
    const styles = await page.evaluate(() => {
      const form = document.querySelector<HTMLElement>(".connection-form")!;
      const url = document.querySelector<HTMLInputElement>("#daemon-url")!;
      const token = document.querySelector<HTMLInputElement>("#auth-token")!;
      const select = document.querySelector<HTMLSelectElement>("#workspace-select")!;
      const probe = document.createElement("div");
      probe.style.background = "var(--neoism-bg-elevated)";
      document.body.appendChild(probe);
      const elevated = getComputedStyle(probe).backgroundColor;
      probe.remove();
      return {
        formBackground: getComputedStyle(form).backgroundColor,
        formShadow: getComputedStyle(form).boxShadow,
        elevated,
        urlBackground: getComputedStyle(url).backgroundColor,
        tokenBackground: getComputedStyle(token).backgroundColor,
        selectBackground: getComputedStyle(select).backgroundColor,
      };
    });

    expect(styles.formBackground).toBe("rgba(0, 0, 0, 0)");
    expect(styles.formShadow).toBe("none");
    expect(styles.urlBackground).toBe(styles.elevated);
    expect(styles.tokenBackground).toBe(styles.elevated);
    expect(styles.selectBackground).toBe(styles.elevated);
    await expect(page.locator(".connection-wordmark")).toHaveAttribute(
      "aria-label",
      "Neoism",
    );

    await page.locator("#daemon-url").focus();
    expect(
      await page.locator("#daemon-url").evaluate((node) =>
        getComputedStyle(node).boxShadow,
      ),
    ).not.toBe("none");
  });

  test("stays within a phone viewport", async ({ page }) => {
    await page.setViewportSize({ width: 375, height: 667 });
    const bounds = await page.locator(".connection-form").boundingBox();
    expect(bounds).not.toBeNull();
    expect(bounds!.x).toBeGreaterThanOrEqual(16);
    expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(375 - 16);
  });
});