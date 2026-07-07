import os

def count_lines(root_dir, extensions):
    total_lines = 0
    file_count = 0
    
    for root, dirs, files in os.walk(root_dir):
        # Ignore 'target' directory
        if 'target' in dirs:
            dirs.remove('target')
            
        for file in files:
            if file.endswith(tuple(extensions)):
                file_path = os.path.join(root, file)
                try:
                    with open(file_path, 'r', encoding='utf-8') as f:
                        lines = len(f.readlines())
                        total_lines += lines
                        file_count += 1
                except Exception as e:
                    print(f"Error reading {file_path}: {e}")
                    
    return total_lines, file_count

if __name__ == "__main__":
    project_root = "."
    extensions = [".rs", ".zig"]
    
    total_lines, file_count = count_lines(project_root, extensions)
    
    print(f"Project: {os.path.abspath(project_root)}")
    print(f"Files processed: {file_count}")
    print(f"Total lines of code ({', '.join(extensions)}): {total_lines}")
