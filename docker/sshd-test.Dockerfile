FROM alpine:latest

# 1. Install OpenSSH
RUN apk update && apk add --no-cache openssh

# 2. Generate host keys (Required for Alpine OpenSSH)
RUN ssh-keygen -A

# 3. Set a root password
RUN echo 'root:rootpassword' | chpasswd

# 4. Allow root login with password
RUN sed -i 's/#PermitRootLogin prohibit-password/PermitRootLogin yes/' /etc/ssh/sshd_config

EXPOSE 22

# 5. Run the SSH daemon in the foreground
CMD ["/usr/sbin/sshd", "-D"]