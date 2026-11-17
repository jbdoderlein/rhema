You can view images by piping bytes into `see`, or by passing file paths:

    curl -sL https://example.com/photo.png | see
    see screenshots/capture.png

Staged images attach to that bash call's result and you see them as images. Formats: png, jpeg, gif, webp; up to 5 MB per image.