ARG DISTRIBUTION=ubuntu:22.04
FROM ${DISTRIBUTION}
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    python3 binutils xvfb xauth xdotool imagemagick fonts-dejavu-core \
    libgl1-mesa-dri libglx-mesa0 \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /workspace
