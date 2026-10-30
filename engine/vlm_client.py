"""
Agent Aura - Unified VLM Gateway Client (Phase 4 implementation)
Connects to Claude 3.7 Computer Use / Qwen2.5-VL / Gemini 2.0 Flash
using single-pass decision-and-grounding to eliminate dual-turn spotter latency.
"""
import os
import json
import base64
import asyncio
from typing import Dict, Any, Optional, List
from engine.batcher import ActionBatch, parse_and_validate_action_batch

class VlmClient:
    """Unified vision-action API gateway supporting Claude 3.7, Gemini 2.0 Flash, and Qwen2.5-VL."""

    def __init__(
        self,
        provider: str = "gemini",
        model_name: Optional[str] = None,
        api_key: Optional[str] = None,
    ):
        self.provider = provider.lower()
        if self.provider == "anthropic" or self.provider == "claude":
            self.model_name = model_name or "claude-3-7-sonnet-20250219"
            self.api_key = api_key or os.getenv("ANTHROPIC_API_KEY", "")
        elif self.provider == "openai" or self.provider == "qwen":
            self.model_name = model_name or "qwen2.5-vl-72b-instruct"
            self.api_key = api_key or os.getenv("OPENAI_API_KEY", os.getenv("DASHSCOPE_API_KEY", ""))
        else: # default to gemini
            self.provider = "gemini"
            self.model_name = model_name or "gemini-2.0-flash"
            self.api_key = api_key or os.getenv("GEMINI_API_KEY", "")

    async def predict_step(
        self,
        image_bytes: bytes,
        user_prompt: str,
        system_prompt: str,
        history: Optional[List[Dict[str, Any]]] = None,
    ) -> ActionBatch:
        """Sends visual frame and prompt to vision model and parses the resulting ActionBatch."""
        b64_image = base64.b64encode(image_bytes).decode("utf-8")

        if self.provider == "gemini":
            response_json_text = await self._call_gemini(b64_image, user_prompt, system_prompt, history)
        elif self.provider in ("anthropic", "claude"):
            response_json_text = await self._call_anthropic(b64_image, user_prompt, system_prompt, history)
        else:
            response_json_text = await self._call_openai_compatible(b64_image, user_prompt, system_prompt, history)

        return parse_and_validate_action_batch(response_json_text)

    async def _call_gemini(
        self,
        b64_image: str,
        user_prompt: str,
        system_prompt: str,
        history: Optional[List[Dict[str, Any]]],
    ) -> str:
        """Dispatches request to Google Gemini API with response_mime_type=application/json."""
        url = f"https://generativelanguage.googleapis.com/v1beta/models/{self.model_name}:generateContent?key={self.api_key}"

        payload = {
            "system_instruction": {
                "parts": [{"text": system_prompt}]
            },
            "contents": [
                {
                    "role": "user",
                    "parts": [
                        {"text": user_prompt},
                        {
                            "inline_data": {
                                "mime_type": "image/webp",
                                "data": b64_image,
                            }
                        },
                    ],
                }
            ],
            "generationConfig": {
                "response_mime_type": "application/json",
                "temperature": 0.0,
            },
        }

        return await self._async_post_json(url, payload, extract_fn=self._extract_gemini_content)

    async def _call_anthropic(
        self,
        b64_image: str,
        user_prompt: str,
        system_prompt: str,
        history: Optional[List[Dict[str, Any]]],
    ) -> str:
        """Dispatches request to Anthropic Claude Messages API."""
        url = "https://api.anthropic.com/v1/messages"
        headers = {
            "x-api-key": self.api_key,
            "anthropic-version": "2023-06-01",
            "content-type": "application/json",
        }

        payload = {
            "model": self.model_name,
            "system": system_prompt,
            "max_tokens": 1024,
            "temperature": 0.0,
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": "image/webp",
                                "data": b64_image,
                            },
                        },
                        {"type": "text", "text": user_prompt},
                    ],
                }
            ],
        }

        return await self._async_post_json(url, payload, headers=headers, extract_fn=self._extract_anthropic_content)

    async def _call_openai_compatible(
        self,
        b64_image: str,
        user_prompt: str,
        system_prompt: str,
        history: Optional[List[Dict[str, Any]]],
    ) -> str:
        """Dispatches request to OpenAI / Qwen compatible endpoint."""
        url = os.getenv("OPENAI_BASE_URL", "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions")
        headers = {
            "Authorization": f"Bearer {self.api_key}",
            "Content-Type": "application/json",
        }

        payload = {
            "model": self.model_name,
            "response_format": {"type": "json_object"},
            "messages": [
                {"role": "system", "content": system_prompt},
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": user_prompt},
                        {
                            "type": "image_url",
                            "image_url": {"url": f"data:image/webp;base64,{b64_image}"},
                        },
                    ],
                },
            ],
        }

        return await self._async_post_json(url, payload, headers=headers, extract_fn=self._extract_openai_content)

    async def _async_post_json(
        self,
        url: str,
        payload: Dict[str, Any],
        headers: Optional[Dict[str, str]] = None,
        extract_fn=None,
    ) -> str:
        """Performs async HTTP POST with fallback to urllib thread executor."""
        headers = headers or {"Content-Type": "application/json"}
        req_data = json.dumps(payload).encode("utf-8")

        def _blocking_post():
            import urllib.request
            req = urllib.request.Request(url, data=req_data, headers=headers, method="POST")
            with urllib.request.urlopen(req, timeout=30) as resp:
                return resp.read().decode("utf-8")

        loop = asyncio.get_event_loop()
        raw_resp = await loop.run_in_executor(None, _blocking_post)

        data = json.loads(raw_resp)
        if extract_fn:
            return extract_fn(data)
        return raw_resp

    def _extract_gemini_content(self, data: Dict[str, Any]) -> str:
        candidates = data.get("candidates", [])
        if not candidates:
            raise ValueError(f"Empty candidates from Gemini: {data}")
        parts = candidates[0].get("content", {}).get("parts", [])
        if not parts:
            raise ValueError(f"Empty parts in candidate: {candidates[0]}")
        return parts[0].get("text", "{}")

    def _extract_anthropic_content(self, data: Dict[str, Any]) -> str:
        content_list = data.get("content", [])
        for block in content_list:
            if block.get("type") == "text":
                return block.get("text", "{}")
        raise ValueError(f"No text block in Anthropic response: {data}")

    def _extract_openai_content(self, data: Dict[str, Any]) -> str:
        choices = data.get("choices", [])
        if not choices:
            raise ValueError(f"Empty choices in OpenAI response: {data}")
        return choices[0].get("message", {}).get("content", "{}")
