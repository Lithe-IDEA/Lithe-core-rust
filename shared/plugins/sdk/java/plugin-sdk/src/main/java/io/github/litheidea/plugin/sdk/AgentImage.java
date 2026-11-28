package io.github.litheidea.plugin.sdk;

/**
 * One image of a prompt: the type Core read from the file's first bytes and
 * the file's absolute path. The file may be gone by the time the backend
 * reads it, so bound what you read.
 */
public final class AgentImage {
    private final String mimeType;
    private final String path;

    AgentImage(String mimeType, String path) {
        this.mimeType = mimeType;
        this.path = path;
    }

    /** {@code image/png}, {@code image/jpeg}, {@code image/gif} or {@code image/webp}. */
    public String mimeType() {
        return mimeType;
    }

    /** The absolute path of the image file. */
    public String path() {
        return path;
    }
}
